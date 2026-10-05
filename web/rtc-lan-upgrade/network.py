"""Browser-side TURN, delayed real-name mDNS answer, and wire evidence."""

import asyncio
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import time

from turn import COOKIE, Turn, attributes
from isolation import require_private_namespace

PHONE_IP = os.environ.get("BUILD_RTC_LAN_PHONE_IP", "10.72.0.2")

def save_report(artifacts, report):
    temporary = artifacts / "wire.json.tmp"
    temporary.write_text(json.dumps(report, indent=2))
    temporary.replace(artifacts / "wire.json")


def question(packet):
    if len(packet) < 12 or packet[2] & 0x80:
        return None
    offset = 12
    labels = []
    while offset < len(packet) and packet[offset]:
        length = packet[offset]
        labels.append(packet[offset + 1 : offset + 1 + length])
        offset += length + 1
    if offset + 5 > len(packet):
        return None
    kind, klass = struct.unpack_from("!HH", packet, offset + 1)
    name = b".".join(labels).decode("ascii", errors="ignore")
    return name, kind, klass


def answer(name):
    encoded = b"".join(bytes([len(label)]) + label.encode() for label in name.split(".")) + b"\0"
    return (struct.pack("!6H", 0, 0x8400, 0, 1, 0, 0) + encoded
            + struct.pack("!HHIH", 1, 0x8001, 120, 4) + socket.inet_aton("10.72.0.2"))


class Mdns(asyncio.DatagramProtocol):
    def __init__(self, report, artifacts):
        self.report = report
        self.artifacts = artifacts
        self.names = set()
        self.transport = None

    def connection_made(self, transport):
        self.transport = transport

    def datagram_received(self, packet, peer):
        query = question(packet)
        if peer[0] != "10.72.0.1" or query is None:
            return
        name, kind, klass = query
        self.report["queries"].append({"name": name, "port": peer[1], "class": klass})
        save_report(self.artifacts, self.report)
        if kind == 1 and name.endswith(".local"):
            self.names.add(name)

    def release(self):
        assert self.names, "the bridge must have queried Chromium's name before release"
        for name in self.names:
            self.transport.sendto(answer(name), ("224.0.0.251", 5353))


async def capture(report, artifacts):
    wire = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(0x0003))
    wire.bind(("eth0", 0))
    wire.setblocking(False)
    loop = asyncio.get_running_loop()
    while True:
        packet = await loop.sock_recv(wire, 65536)
        if len(packet) < 42 or packet[12:14] != b"\x08\x00" or packet[23] != 17:
            continue
        offset = 14 + (packet[14] & 15) * 4
        source_port, destination_port = struct.unpack_from("!HH", packet, offset)
        payload = packet[offset + 8 :]
        if len(payload) < 20 or struct.unpack_from("!I", payload, 4)[0] != COOKIE:
            continue
        host_file = artifacts / "host.json"
        if not host_file.exists():
            continue
        host_ports = {host["port"] for host in json.loads((artifacts / "gathered-hosts.json").read_text())}
        kind = struct.unpack_from("!H", payload)[0]
        if (packet[26:30] == socket.inet_aton("10.72.0.1")
                and packet[30:34] == socket.inet_aton(PHONE_IP) and destination_port in host_ports):
            key = {0x0001: "host_checks", 0x0011: "host_socket_indications"}.get(kind)
        elif (packet[26:30] == socket.inet_aton(PHONE_IP)
              and packet[30:34] == socket.inet_aton("10.72.0.1") and source_port in host_ports and kind == 0x0001):
            key = "browser_checks"
        else:
            continue
        if key is None:
            continue
        fields = attributes(payload)
        report[key].append({
            "at": time.time(), "source_port": source_port, "destination_port": destination_port,
            "transaction": payload[8:20].hex(), "after_release": report["released"],
            "bytes": len(payload), "has_integrity": 0x0008 in fields,
            "has_username": 0x0006 in fields, "attributes": len(fields),
            "has_fingerprint": 0x8028 in fields,
        })
        save_report(artifacts, report)


async def main(artifacts):
    require_private_namespace()
    mode = os.environ.get("BUILD_RTC_LAN_MODE", "delayed")
    report = {"queries": [], "host_checks": [], "browser_checks": [],
              "host_socket_indications": [], "released": False, "mdns_silenced": mode != "delayed"}
    save_report(artifacts, report)
    loop = asyncio.get_running_loop()
    await loop.create_datagram_endpoint(Turn, local_addr=("198.18.0.1", 3478))
    mdns_socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    mdns_socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    mdns_socket.bind(("224.0.0.251", 5353))
    mdns_socket.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP,
                          socket.inet_aton("224.0.0.251") + socket.inet_aton(PHONE_IP))
    mdns_socket.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(PHONE_IP))
    _, mdns = await loop.create_datagram_endpoint(lambda: Mdns(report, artifacts), sock=mdns_socket)
    asyncio.create_task(capture(report, artifacts))
    (artifacts / "network-ready").write_text("ready")
    # Chromium first registers its UUID normally. Before that candidate is
    # signaled to the bridge, hold its answers so resolution remains pending.
    while not (artifacts / "gate").exists():
        await asyncio.sleep(0.02)
    subprocess.run(["nft", "add", "rule", "inet", "delay_mdns", "output",
                    "udp", "sport", "5353", "drop"], check=True)
    host_port = json.loads((artifacts / "host.json").read_text())["port"]
    if mode in {"delayed", "far-edge-pressure"}:
        subprocess.run(["nft", "add", "rule", "inet", "hold_checks", "output",
                        "ip", "daddr", "10.72.0.1", "udp", "sport", str(host_port), "counter", "drop"], check=True)
    (artifacts / "gated").write_text("ready")
    while not (artifacts / "release").exists():
        await asyncio.sleep(0.02)
    report["released"] = True
    if mode == "delayed":
        subprocess.run(["nft", "delete", "table", "inet", "delay_mdns"], check=True)
        mdns.release()
    save_report(artifacts, report)
    if mode != "delayed":
        while True:
            await asyncio.sleep(0.1)
    # The browser can receive the bridge's checks, but its host socket cannot
    # answer until the session has completed a full pull over the TURN pair.
    while not (artifacts / "release-host-checks").exists():
        await asyncio.sleep(0.02)
    subprocess.run(["nft", "delete", "table", "inet", "hold_checks"], check=True)
    while not (artifacts / "disconnect-host").exists():
        await asyncio.sleep(0.02)
    # The bridge keeps sending its authenticated checks, so Chromium still
    # hears it. Drop every UDP return to make the bridge itself reach Failed
    # without replacing ICE's production timeout or inventing an event.
    subprocess.run(["nft", "-f", "-"], input="""
table inet fail_bridge_ice {
 chain output { type filter hook output priority 0; policy accept;
   ip daddr 10.72.0.1 ip protocol udp drop
 }
}
""", text=True, check=True)
    (artifacts / "host-disconnected").write_text("ready")
    while True:
        await asyncio.sleep(0.1)


if __name__ == "__main__":
    asyncio.run(main(Path(sys.argv[1])))
