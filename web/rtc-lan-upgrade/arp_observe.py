#!/usr/bin/env python3
"""Capture independent ARP, UDP/9, STUN, and neighbor evidence on fixture eth0."""

import json
import os
from pathlib import Path
import signal
import socket
import struct
import subprocess
import sys
import time

from isolation import require_private_namespace


BRIDGE_IP = "10.72.0.1"
STUN_COOKIE = 0x2112A442
STUN_KINDS = {0x0001: "binding_request", 0x0011: "binding_indication",
              0x0101: "binding_success", 0x0111: "binding_error"}
SO_TIMESTAMPNS_NEW = 64


def mac_address(value):
    return ":".join(f"{part:02x}" for part in value)


def direction(source_ip, target_ip, side, phone_ip):
    local_ip = BRIDGE_IP if side == "bridge" else phone_ip
    return "outbound" if source_ip == local_ip else "inbound" if target_ip == local_ip else None


def decode_stun(payload):
    if len(payload) < 20:
        return None
    kind, length, cookie = struct.unpack_from("!HHI", payload)
    if kind & 0xC000 or length % 4 or cookie != STUN_COOKIE or len(payload) != 20 + length:
        return None
    fields = set()
    offset = 20
    while offset < len(payload):
        if offset + 4 > len(payload):
            return None
        attribute, size = struct.unpack_from("!HH", payload, offset)
        offset += 4
        padded_size = (size + 3) & ~3
        if offset + padded_size > len(payload):
            return None
        if (attribute == 0x0008 and size != 20
                or attribute == 0x001C and size != 32
                or attribute == 0x8028 and size != 4):
            return None
        fields.add(attribute)
        offset += padded_size
    has_integrity = 0x0008 in fields or 0x001C in fields
    has_username = 0x0006 in fields
    return {"stun_kind": STUN_KINDS.get(kind, f"0x{kind:04x}"),
            "transaction": payload[8:20].hex(),
            "has_integrity": has_integrity, "has_username": has_username,
            "has_fingerprint": 0x8028 in fields,
            "request_with_credentials": kind == 0x0001 and has_username and has_integrity}


def decode_arp(frame, side, phone_ip, macs):
    if len(frame) < 42:
        return None
    hardware, protocol, hardware_size, protocol_size, operation = struct.unpack_from("!HHBBH", frame, 14)
    if (hardware, protocol, hardware_size, protocol_size) != (1, 0x0800, 6, 4):
        return None
    if operation not in (1, 2):
        return None
    source_ip = socket.inet_ntoa(frame[28:32])
    target_ip = socket.inet_ntoa(frame[38:42])
    if {source_ip, target_ip} != {BRIDGE_IP, phone_ip}:
        return None
    return {"kind": "arp", "direction": direction(source_ip, target_ip, side, phone_ip),
            **macs, "operation": "request" if operation == 1 else "reply",
            "source_ip": source_ip, "target_ip": target_ip,
            "arp_source_mac": mac_address(frame[22:28]),
            "arp_target_mac": mac_address(frame[32:38])}


def decode_udp(frame, side, phone_ip, macs):
    if len(frame) < 34 or frame[14] >> 4 != 4:
        return None
    header_size = (frame[14] & 15) * 4
    total_size = struct.unpack_from("!H", frame, 16)[0]
    if (header_size < 20 or len(frame) < 14 + header_size + 8
            or total_size < header_size + 8 or len(frame) < 14 + total_size
            or frame[23] != 17 or struct.unpack_from("!H", frame, 20)[0] & 0x3FFF):
        return None
    source_ip = socket.inet_ntoa(frame[26:30])
    target_ip = socket.inet_ntoa(frame[30:34])
    udp_offset = 14 + header_size
    source_port, destination_port, udp_size = struct.unpack_from("!HHH", frame, udp_offset)
    if udp_size < 8 or udp_size > total_size - header_size:
        return None
    payload = frame[udp_offset + 8:udp_offset + udp_size]
    common = {**macs, "direction": direction(source_ip, target_ip, side, phone_ip),
              "source_ip": source_ip, "target_ip": target_ip,
              "source_port": source_port, "destination_port": destination_port,
              "bytes": len(payload)}
    if source_ip == BRIDGE_IP and destination_port == 9:
        return {"kind": "scout", **common}
    if {source_ip, target_ip} != {BRIDGE_IP, phone_ip}:
        return None
    stun = decode_stun(payload)
    return {"kind": "stun", **common, **stun} if stun else None


def decode_frame(frame, side, phone_ip):
    if len(frame) < 14:
        return None
    macs = {"source_mac": mac_address(frame[6:12]),
            "destination_mac": mac_address(frame[:6])}
    protocol = struct.unpack_from("!H", frame, 12)[0]
    if protocol == 0x0806:
        return decode_arp(frame, side, phone_ip, macs)
    if protocol == 0x0800:
        return decode_udp(frame, side, phone_ip, macs)
    return None


def neighbor_snapshot(stream, previous):
    before = time.time()
    result = subprocess.run(["ip", "-j", "neigh", "show", "dev", "eth0"],
                            text=True, capture_output=True, check=True)
    after = time.time()
    neighbors = json.loads(result.stdout)
    current = json.dumps(neighbors, sort_keys=True)
    if current != previous:
        stream.write(json.dumps({"before": before, "after": after,
                                 "neighbors": neighbors}) + "\n")
        stream.flush()
    return current


def write_pcap_packet(stream, packet, captured_ns):
    seconds, nanoseconds = divmod(captured_ns, 1_000_000_000)
    stream.write(struct.pack("<IIII", seconds, nanoseconds // 1000,
                             len(packet), len(packet)))
    stream.write(packet)
    stream.flush()


def packet_timestamp(ancillary):
    for level, kind, data in ancillary:
        if level == socket.SOL_SOCKET and kind == SO_TIMESTAMPNS_NEW and len(data) >= 16:
            seconds, nanoseconds = struct.unpack_from("=qq", data)
            if seconds >= 0 and 0 <= nanoseconds < 1_000_000_000:
                return seconds * 1_000_000_000 + nanoseconds, "kernel_socket_timestamp"
    return time.time_ns(), "userspace_receive"


def observe(artifacts, side):
    require_private_namespace()
    if side not in ("bridge", "phone"):
        raise ValueError("side must be bridge or phone")
    artifacts = Path(artifacts)
    artifacts.mkdir(parents=True, exist_ok=True)
    phone_ip = os.environ.get("BUILD_RTC_LAN_PHONE_IP", "10.72.3.254")
    running = True

    def stop(_signal, _frame):
        nonlocal running
        running = False

    signal.signal(signal.SIGTERM, stop)
    wire = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(0x0003))
    monitor = None
    try:
        wire.bind(("eth0", 0))
        wire.setsockopt(socket.SOL_SOCKET, SO_TIMESTAMPNS_NEW, 1)
        wire.settimeout(0.02)
        with ((artifacts / f"{side}-observe.pcap").open("wb") as pcap,
              (artifacts / f"{side}-observe.jsonl").open("w", buffering=1) as events,
              (artifacts / f"{side}-ip-neigh.jsonl").open("w", buffering=1) as neighbors,
              (artifacts / f"{side}-ip-monitor.log").open("w", buffering=1) as monitor_log):
            pcap.write(struct.pack("<IHHIIII", 0xA1B2C3D4, 2, 4, 0, 0, 65535, 1))
            pcap.flush()
            previous = neighbor_snapshot(neighbors, None)
            monitor = subprocess.Popen(["ip", "-ts", "monitor", "neigh", "dev", "eth0"],
                                       stdout=monitor_log, stderr=subprocess.STDOUT)
            (artifacts / f"{side}-observe-ready").write_text("ready")
            next_sample = time.monotonic() + 0.02
            while running:
                try:
                    packet, ancillary, _flags, _address = wire.recvmsg(65535, socket.CMSG_SPACE(16))
                except socket.timeout:
                    packet = None
                if packet is not None:
                    captured_ns, timestamp_source = packet_timestamp(ancillary)
                    write_pcap_packet(pcap, packet, captured_ns)
                    event = decode_frame(packet, side, phone_ip)
                    if event is not None:
                        events.write(json.dumps({"at": captured_ns / 1_000_000_000,
                                                 "timestamp_source": timestamp_source,
                                                 "side": side, **event}) + "\n")
                if time.monotonic() >= next_sample:
                    previous = neighbor_snapshot(neighbors, previous)
                    next_sample = time.monotonic() + 0.02
    finally:
        wire.close()
        if monitor is not None:
            monitor.terminate()
            monitor.wait()


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[2] not in ("bridge", "phone"):
        raise SystemExit("usage: arp_observe.py ARTIFACTS_DIR bridge|phone")
    observe(sys.argv[1], sys.argv[2])
