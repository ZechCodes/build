"""Packet-level evidence for the private LAN investigation helper."""

import io
import json
import socket
import struct
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import arp_observe


BRIDGE = "10.72.0.1"
PHONE = "10.72.3.254"
BRIDGE_MAC = bytes.fromhex("020000000001")
PHONE_MAC = bytes.fromhex("020000000002")


def ethernet(payload, kind, source=BRIDGE_MAC, destination=PHONE_MAC):
    return destination + source + struct.pack("!H", kind) + payload


def arp(operation, source_ip=BRIDGE, target_ip=PHONE):
    body = struct.pack("!HHBBH", 1, 0x0800, 6, 4, operation)
    body += BRIDGE_MAC + socket.inet_aton(source_ip)
    body += bytes(6) + socket.inet_aton(target_ip)
    return ethernet(body, 0x0806)


def stun(kind, attributes=(), declared_length=None):
    body = b"".join(struct.pack("!HH", typ, len(value)) + value
                    + bytes((-len(value)) % 4) for typ, value in attributes)
    return (struct.pack("!HHI", kind, len(body) if declared_length is None else declared_length,
                        0x2112A442) + bytes.fromhex("00112233445566778899aabb") + body)


def udp(payload, source_ip=BRIDGE, target_ip=PHONE, source_port=5000, target_port=5001):
    datagram = struct.pack("!HHHH", source_port, target_port, 8 + len(payload), 0) + payload
    ip = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(datagram), 0, 0, 64, 17, 0,
                     socket.inet_aton(source_ip), socket.inet_aton(target_ip))
    return ethernet(ip + datagram, 0x0800)


class PacketDecoderTests(unittest.TestCase):
    def decode(self, packet):
        return arp_observe.decode_frame(packet, "bridge", PHONE)

    def test_arp_request_and_reply_keep_operation_addresses_and_macs(self):
        request = self.decode(arp(1))
        reply = self.decode(arp(2, PHONE, BRIDGE))
        self.assertEqual(request["kind"], "arp")
        self.assertEqual(request["operation"], "request")
        self.assertEqual(request["source_ip"], BRIDGE)
        self.assertEqual(request["target_ip"], PHONE)
        self.assertEqual(request["source_mac"], "02:00:00:00:00:01")
        self.assertEqual(request["destination_mac"], "02:00:00:00:00:02")
        self.assertEqual(request["direction"], "outbound")
        self.assertEqual(reply["operation"], "reply")
        self.assertEqual(reply["direction"], "inbound")
        self.assertIsNone(self.decode(arp(1, BRIDGE, "10.72.2.7")))

    def test_short_or_invalid_ethernet_arp_ipv4_and_udp_are_ignored(self):
        valid_arp = arp(1)
        valid_udp = udp(stun(0x0011))
        for packet in (b"", valid_arp[:13], valid_arp[:-1],
                       valid_arp[:14] + bytes.fromhex("0002080006040001") + valid_arp[22:],
                       valid_udp[:33], valid_udp[:-1],
                       valid_udp[:14] + bytes.fromhex("4600") + valid_udp[16:]):
            with self.subTest(packet=packet.hex()):
                self.assertIsNone(self.decode(packet))

    def test_indication_is_distinct_from_request_with_credential_attributes(self):
        credentials = [(0x0006, b"ufrag:remote"), (0x0008, bytes(20)),
                       (0x8028, bytes(4))]
        indication = self.decode(udp(stun(0x0011, [(0x8028, bytes(4))])))
        request = self.decode(udp(stun(0x0001, credentials)))
        self.assertEqual(indication["kind"], "stun")
        self.assertEqual(indication["stun_kind"], "binding_indication")
        self.assertFalse(indication["request_with_credentials"])
        self.assertFalse(indication["has_username"])
        self.assertFalse(indication["has_integrity"])
        self.assertEqual(request["stun_kind"], "binding_request")
        self.assertTrue(request["request_with_credentials"])
        self.assertTrue(request["has_fingerprint"])
        self.assertEqual(request["transaction"], "00112233445566778899aabb")

    def test_binding_success_retains_transaction_without_claiming_a_request(self):
        response = self.decode(udp(stun(0x0101, [(0x8028, bytes(4))]),
                                   source_ip=PHONE, target_ip=BRIDGE))
        self.assertEqual(response["stun_kind"], "binding_success")
        self.assertEqual(response["transaction"], "00112233445566778899aabb")
        self.assertFalse(response["request_with_credentials"])

    def test_forged_or_truncated_stun_attributes_are_rejected(self):
        valid = stun(0x0001, [(0x0006, b"name"), (0x0008, bytes(20))])
        invalid = [valid[:19], valid[:-1], valid + struct.pack("!HH", 0x0008, 0),
                   stun(0x0001, [(0x0008, bytes(19))]),
                   stun(0x0001, [(0x8028, bytes(3))]),
                   stun(0x0001, [(0x0008, bytes(20))], declared_length=4),
                   stun(0x0001, [(0x0006, b"name")], declared_length=7)]
        for payload in invalid:
            with self.subTest(payload=payload.hex()):
                self.assertIsNone(self.decode(udp(payload)))

    def test_bridge_udp_port_nine_scout_is_saved_even_without_stun(self):
        row = self.decode(udp(b"probe", target_ip="10.72.2.9", target_port=9))
        self.assertEqual(row["kind"], "scout")
        self.assertEqual(row["target_ip"], "10.72.2.9")
        self.assertEqual(row["destination_port"], 9)
        self.assertEqual(row["bytes"], 5)
        self.assertIsNone(self.decode(udp(b"probe", source_ip=PHONE, target_ip=BRIDGE,
                                           target_port=9)))


class StartupTests(unittest.TestCase):
    def test_namespace_guard_runs_before_socket_or_process(self):
        with (patch.object(arp_observe, "require_private_namespace",
                           side_effect=AssertionError("unsafe")) as guard,
              patch.object(arp_observe.socket, "socket") as open_socket,
              patch.object(arp_observe.subprocess, "run") as run):
            with self.assertRaisesRegex(AssertionError, "unsafe"):
                arp_observe.observe("/tmp/unused", "bridge")
            guard.assert_called_once_with()
            open_socket.assert_not_called()
            run.assert_not_called()


class ArtifactTests(unittest.TestCase):
    def test_neighbor_timeline_keeps_initial_empty_state_and_only_changes(self):
        stream = io.StringIO()
        outputs = ["[]", "[]", '[{"dst":"10.72.3.254","state":["REACHABLE"]}]']
        with (patch.object(arp_observe.subprocess, "run",
                           side_effect=[SimpleNamespace(stdout=output) for output in outputs]) as run,
              patch.object(arp_observe.time, "time", side_effect=range(100, 106))):
            previous = None
            for _ in outputs:
                previous = arp_observe.neighbor_snapshot(stream, previous)
        rows = [json.loads(line) for line in stream.getvalue().splitlines()]
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0], {"before": 100, "after": 101, "neighbors": []})
        self.assertEqual(rows[1]["before"], 104)
        self.assertEqual(rows[1]["after"], 105)
        self.assertEqual(rows[1]["neighbors"][0]["dst"], PHONE)
        self.assertEqual(run.call_count, 3)

    def test_pcap_record_has_timestamp_and_original_packet_length(self):
        stream = io.BytesIO()
        arp_observe.write_pcap_packet(stream, b"abc", 1_234_567_890)
        self.assertEqual(struct.unpack("<IIII", stream.getvalue()[:16]),
                         (1, 234567, 3, 3))
        self.assertEqual(stream.getvalue()[16:], b"abc")

    def test_kernel_timestamp_is_used_when_present(self):
        ancillary = [(socket.SOL_SOCKET, arp_observe.SO_TIMESTAMPNS_NEW,
                      struct.pack("=qq", 123, 456_000_000))]
        self.assertEqual(arp_observe.packet_timestamp(ancillary),
                         (123_456_000_000, "kernel_socket_timestamp"))


if __name__ == "__main__":
    unittest.main()
