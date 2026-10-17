from __future__ import annotations

import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.errors import LauncherError
from nivora_launcher.server import _apply_server_port
from nivora_launcher.server_network import ServerPublisher, diagnose, nat_kind, port_number, properties_info, public_ipv4
from nivora_launcher.upnp import Gateway, UpnpError, gateways_from_description, parse_xml, router_request, router_url


class FakeGateway:
    peer = "192.168.1.1"
    local_ip = "192.168.1.25"
    name = "Test Router"
    wan = "8.8.4.4"

    def __init__(self):
        self.entries = {}
        self.calls = []
        self.lease = 600
        self.fail_delete = False

    def external_ip(self):
        return self.wan

    def mapping(self, port):
        return self.entries.get(port)

    def add(self, external, internal, description, lease):
        self.calls.append(("add", external, internal, lease))
        self.entries[external] = {"NewInternalClient": self.local_ip, "NewInternalPort": str(internal),
                                  "NewPortMappingDescription": description, "NewLeaseDuration": str(self.lease),
                                  "NewEnabled": "1"}

    def delete(self, port):
        self.calls.append(("delete", port))
        if self.fail_delete:
            raise UpnpError("Роутер недоступен")
        self.entries.pop(port, None)


class ServerNetworkTests(unittest.TestCase):
    def setUp(self):
        self.root = Path("/tmp/trial-network-unit-server")
        self.gateway = FakeGateway()
        self.publisher = ServerPublisher()
        self.running = True
        self.patches = [patch("nivora_launcher.server_network.tcp_local", return_value=True),
                        patch("nivora_launcher.server_network.threading.Thread.start")]
        for item in self.patches:
            item.start()

    def tearDown(self):
        self.gateway.fail_delete = False
        self.publisher.shutdown()
        for item in self.patches:
            item.stop()

    def open(self, **kwargs):
        values = {"running": lambda: self.running, "confirmed": True,
                  "info": {"bind_all": True, "observed_ip": "8.8.4.4"}, "gateway": self.gateway}
        values.update(kwargs)
        return self.publisher.open(self.root, 25565, 25570, **values)

    def test_only_confirmed_publication_is_allowed(self):
        with self.assertRaisesRegex(LauncherError, "Подтвердите"):
            self.open(confirmed=False)
        self.assertEqual(self.gateway.calls, [])

    def test_requires_running_server(self):
        self.running = False
        with self.assertRaisesRegex(LauncherError, "запустите"):
            self.open()
        self.assertEqual(self.gateway.calls, [])

    def test_rejects_restricted_bind_address(self):
        with self.assertRaisesRegex(LauncherError, "server-ip"):
            self.open(info={"bind_all": False})

    def test_checks_the_actual_local_listener_before_opening(self):
        with patch("nivora_launcher.server_network.tcp_local", return_value=False):
            with self.assertRaisesRegex(LauncherError, "TCP-порт"):
                self.open()
        self.assertEqual(self.gateway.calls, [])

    def test_mapping_is_temporary_tcp_and_idempotent(self):
        result = self.open()
        self.assertTrue(result["active"])
        self.assertEqual(result["address"], "8.8.4.4:25570")
        self.assertEqual(self.gateway.calls, [("add", 25570, 25565, 600)])
        self.open()
        self.assertEqual(len(self.gateway.calls), 1)
        self.assertLessEqual(result["expires_at"] - time.time(), 600)

    def test_reopening_an_expired_mapping_renews_it_instead_of_reporting_inactive_success(self):
        self.open()
        value = self.publisher._publications[self.publisher._key(self.root)]
        value.expires_at = time.time() - 1

        result = self.open()

        self.assertTrue(result["active"])
        self.assertGreater(result["expires_at"], time.time())
        self.assertEqual(len([call for call in self.gateway.calls if call[0] == "add"]), 2)

    def test_open_does_not_report_a_closing_mapping_as_success(self):
        self.open()
        value = self.publisher._publications[self.publisher._key(self.root)]
        value.closing = True

        with self.assertRaisesRegex(LauncherError, "ещё закрывается"):
            self.open()

    def test_never_overwrites_foreign_rule(self):
        self.gateway.entries[25570] = {"NewInternalClient": "192.168.1.9", "NewInternalPort": "25565",
                                       "NewPortMappingDescription": "Other app"}
        with self.assertRaisesRegex(LauncherError, "занят"):
            self.open()
        self.assertEqual(self.gateway.calls, [])

    def test_does_not_overwrite_a_different_publication(self):
        self.open()
        with self.assertRaisesRegex(LauncherError, "другим сервером"):
            self.publisher.open(Path("/tmp/other-server"), 25565, 25570, running=lambda: True,
                                confirmed=True, info={"bind_all": True}, gateway=self.gateway)
        with self.assertRaisesRegex(LauncherError, "закройте"):
            self.publisher.open(self.root, 25565, 25571, running=lambda: True,
                                confirmed=True, info={"bind_all": True}, gateway=self.gateway)

    def test_never_deletes_a_rule_replaced_outside_launcher(self):
        self.open()
        self.gateway.entries[25570]["NewPortMappingDescription"] = "Other app"
        with self.assertRaisesRegex(LauncherError, "чужое"):
            self.publisher.close(self.root)
        self.assertNotIn(("delete", 25570), self.gateway.calls)

    def test_rejects_cgnat_private_wan_and_double_nat(self):
        for wan in ["100.64.4.2", "10.0.0.1", "192.168.0.2", "8.8.8.8"]:
            self.gateway.wan = wan
            with self.subTest(wan=wan), self.assertRaisesRegex(LauncherError, "NAT"):
                self.open()
        self.assertEqual(self.gateway.calls, [])

    def test_rejects_permanent_router_lease_and_deletes_its_rule(self):
        self.gateway.lease = 0
        with self.assertRaisesRegex(LauncherError, "временное"):
            self.open()
        self.assertEqual(self.gateway.calls[-1], ("delete", 25570))
        self.assertEqual(self.gateway.entries, {})

    def test_renews_while_running_and_deletes_after_exit(self):
        self.open()
        value = self.publisher._publications[self.publisher._key(self.root)]
        self.publisher.tick(value.renew_at + 1)
        self.assertEqual(len([call for call in self.gateway.calls if call[0] == "add"]), 2)
        self.running = False
        self.publisher.tick()
        self.assertFalse(self.publisher.state(self.root)["active"])
        self.assertEqual(self.gateway.calls[-1], ("delete", 25570))

    def test_close_failure_stops_renewal_and_can_be_retried(self):
        self.open()
        self.gateway.fail_delete = True
        with self.assertRaises(UpnpError):
            self.publisher.close(self.root)
        result = self.publisher.state(self.root)
        self.assertTrue(result["closing"])
        self.assertFalse(result["active"])
        self.assertTrue(result["error"])
        self.publisher.tick()
        self.assertEqual(len([call for call in self.gateway.calls if call[0] == "add"]), 1)
        self.gateway.fail_delete = False
        self.publisher.close(self.root)
        self.assertFalse(self.publisher.state(self.root)["active"])

    def test_diagnostics_never_claim_external_verification(self):
        with patch("nivora_launcher.server_network.observed_ipv4", return_value="8.8.4.4"), \
             patch("nivora_launcher.server_network.source_ip", return_value="192.168.1.25"), \
             patch("nivora_launcher.server_network.discover_gateway", return_value=self.gateway):
            result = diagnose(self.root, 25565, "server-ip=\nonline-mode=false\n", True, external_port=25570)
        self.assertFalse(result["external_verified"])
        self.assertTrue(result["local_listening"])
        self.assertEqual(result["nat_kind"], "public")
        self.assertEqual(result["address"], "8.8.4.4:25570")
        self.assertTrue(any("online-mode=false" in item for item in result["warnings"]))
        self.assertEqual(self.gateway.calls, [])

    def test_mismatched_router_wan_is_never_offered_as_a_copyable_address(self):
        self.gateway.wan = "8.8.8.8"
        with patch("nivora_launcher.server_network.observed_ipv4", return_value="8.8.4.4"), \
             patch("nivora_launcher.server_network.source_ip", return_value="192.168.1.25"), \
             patch("nivora_launcher.server_network.discover_gateway", return_value=self.gateway):
            result = diagnose(self.root, 25565, "", True, external_port=25570)
        self.assertEqual(result["nat_kind"], "double_nat")
        self.assertEqual(result["public_ip"], "")
        self.assertEqual(result["address"], "")

    def test_network_diagnostics_degrade_to_manual_without_upnp(self):
        with patch("nivora_launcher.server_network.observed_ipv4", return_value="8.8.4.4"), \
             patch("nivora_launcher.server_network.source_ip", return_value="8.8.4.4"), \
             patch("nivora_launcher.server_network.discover_gateway", side_effect=UpnpError("No IGD")):
            result = diagnose(self.root, 25565, "", False)
        self.assertEqual(result["nat_kind"], "direct")
        self.assertFalse(result["upnp_available"])
        self.assertFalse(result["local_listening"])
        self.assertEqual(result["address"], "8.8.4.4:25565")

    def test_public_ipv4_and_nat_classification(self):
        for value in ["127.0.0.1", "0.0.0.0", "10.2.3.4", "100.127.1.1", "192.0.0.9", "192.0.0.10", "192.168.1.1", "169.254.1.1", "224.0.0.1", "2001:db8::1", "not-an-ip"]:
            self.assertFalse(public_ipv4(value), value)
        self.assertTrue(public_ipv4("8.8.4.4"))
        self.assertEqual(nat_kind("100.64.1.2", "8.8.4.4", "192.168.1.2"), "cgnat")
        self.assertEqual(nat_kind("10.0.0.1", "8.8.4.4", "192.168.1.2"), "private")
        self.assertEqual(nat_kind("8.8.8.8", "8.8.4.4", "192.168.1.2"), "double_nat")
        self.assertEqual(nat_kind("", "", "192.168.1.2"), "unknown")

    def test_ports_are_strictly_validated(self):
        for value in [0, 65536, -2, True, 2.5, "25565; rm -rf /", "abc"]:
            with self.assertRaises(LauncherError, msg=str(value)):
                port_number(value)
        self.assertEqual(port_number("25565"), 25565)

    def test_preparation_keeps_authentication_whitelist_and_unrelated_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            p = root / "server.properties"
            p.write_bytes(b"# test\r\nserver-ip=127.0.0.1\r\nserver-port=1\r\nonline-mode=true\r\nwhite-list=true\r\nmotd=Keep\r\n")
            _apply_server_port(root, 25565, public_bind=True)
            self.assertEqual(p.read_bytes(), b"# test\r\nserver-ip=\r\nserver-port=25565\r\nonline-mode=true\r\nwhite-list=true\r\nmotd=Keep\r\n")
            self.assertTrue(properties_info(p.read_text(), 25565)["bind_all"])
            self.assertTrue(properties_info(p.read_text(), 25565)["whitelist"])

    def test_preparation_refuses_server_properties_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside = root / "outside"
            outside.write_text("unchanged")
            (root / "server.properties").symlink_to(outside)
            with self.assertRaises(LauncherError):
                _apply_server_port(root, 25565, public_bind=True)
            self.assertEqual(outside.read_text(), "unchanged")


class UpnpSecurityTests(unittest.TestCase):
    def test_only_literal_rfc1918_peer_urls_are_accepted(self):
        self.assertEqual(router_url("http://192.168.1.1:1234/control", "192.168.1.1"), ("192.168.1.1", 1234, "/control"))
        for url in ["http://127.0.0.1/", "http://169.254.169.254/", "http://8.8.8.8/", "http://router.local/", "http://192.168.1.2/", "http://user:pass@192.168.1.1/", "file:///etc/passwd", "http://192.168.1.1/#fragment"]:
            with self.assertRaises(UpnpError, msg=url):
                router_url(url, "192.168.1.1")

    def test_xml_entities_and_oversized_xml_are_rejected(self):
        for raw in [b'<!DOCTYPE a [<!ENTITY x SYSTEM "file:///etc/passwd">]><a>&x;</a>', b'<!DOCTYPE a><a />', b"x" * (256 * 1024 + 1)]:
            with self.assertRaises(UpnpError):
                parse_xml(raw)

    def test_description_cannot_move_control_endpoint_to_another_host(self):
        raw = b'<root><device><serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL>http://169.254.169.254/control</controlURL></service></serviceList></device></root>'
        with patch("nivora_launcher.upnp.source_ip", return_value="192.168.1.25"):
            with self.assertRaises(UpnpError):
                gateways_from_description(raw, "http://192.168.1.1/description.xml", "192.168.1.1")

    def test_safe_description_uses_only_a_router_local_urlbase(self):
        raw = b'<root><URLBase>http://8.8.8.8/</URLBase><device><friendlyName>Router</friendlyName><serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:2</serviceType><controlURL>/control</controlURL></service></serviceList></device></root>'
        local_raw = b'<root><URLBase>http://192.168.1.1/custom/</URLBase><device><friendlyName>Router</friendlyName><serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:2</serviceType><controlURL>control</controlURL></service></serviceList></device></root>'
        with patch("nivora_launcher.upnp.source_ip", return_value="192.168.1.25"):
            result = gateways_from_description(raw, "http://192.168.1.1/device.xml", "192.168.1.1")
            local_result = gateways_from_description(local_raw, "http://192.168.1.1/device.xml", "192.168.1.1")
        self.assertEqual(result[0].control_url, "http://192.168.1.1/control")
        self.assertEqual(local_result[0].control_url, "http://192.168.1.1/custom/control")

    def test_redirect_is_rejected_without_following_it(self):
        from unittest.mock import MagicMock
        connection = MagicMock()
        connection.getresponse.return_value.status = 302
        with patch("nivora_launcher.upnp.http.client.HTTPConnection", return_value=connection):
            with self.assertRaisesRegex(UpnpError, "перенаправление"):
                router_request("http://192.168.1.1/device.xml", "192.168.1.1")
        connection.close.assert_called_once()
        connection.request.assert_called_once()

    def test_soap_fault_714_means_no_mapping(self):
        gateway = Gateway("192.168.1.1", "http://192.168.1.1/control", "urn:schemas-upnp-org:service:WANIPConnection:1", "Router", "192.168.1.25")
        fault = b'<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><errorCode>714</errorCode></s:Fault></s:Body></s:Envelope>'
        with patch("nivora_launcher.upnp.router_request", return_value=(500, fault)):
            self.assertIsNone(gateway.mapping(25565))

    def test_soap_add_is_tcp_only_and_uses_finite_lease(self):
        gateway = Gateway("192.168.1.1", "http://192.168.1.1/control", "urn:schemas-upnp-org:service:WANIPConnection:1", "Router", "192.168.1.25")
        response = b'<Envelope><Body><AddPortMappingResponse /></Body></Envelope>'
        with patch("nivora_launcher.upnp.router_request", return_value=(200, response)) as call:
            gateway.add(25570, 25565, "Launcher<&>", 600)
        body = call.call_args.kwargs["body"]
        self.assertIn(b"<NewProtocol>TCP</NewProtocol>", body)
        self.assertIn(b"<NewLeaseDuration>600</NewLeaseDuration>", body)
        self.assertIn(b"Launcher&lt;&amp;&gt;", body)


if __name__ == "__main__":
    unittest.main()
