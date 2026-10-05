"""Small UDP TURN fixture; never listens outside the disposable browser LAN."""

import asyncio
import hashlib
import hmac
import socket
import struct

COOKIE = 0x2112A442
REALM = b"build-lan-fixture"
NONCE = b"isolated-test-nonce"
USERNAME = b"fixture"
PASSWORD = b"fixture-password"
# RFC 5389 long-term TURN integrity keys require MD5 for these fixture credentials.
# nosemgrep: python.lang.security.audit.md5-used-as-password.md5-used-as-password
KEY = hashlib.md5(USERNAME + b":" + REALM + b":" + PASSWORD).digest()


def attribute(kind, value):
    return struct.pack("!HH", kind, len(value)) + value + bytes((-len(value)) % 4)


def attributes(data):
    result = {}
    offset = 20
    while offset + 4 <= len(data):
        kind, length = struct.unpack_from("!HH", data, offset)
        result[kind] = data[offset + 4 : offset + 4 + length]
        offset += 4 + ((length + 3) // 4) * 4
    return result


def xor_address(address):
    host, port = address
    return struct.pack("!BBHI", 0, 1, port ^ (COOKIE >> 16),
                       struct.unpack("!I", socket.inet_aton(host))[0] ^ COOKIE)


def peer_address(value):
    _, family, port, host = struct.unpack("!BBHI", value)
    assert family == 1
    return socket.inet_ntoa(struct.pack("!I", host ^ COOKIE)), port ^ (COOKIE >> 16)


def message(kind, transaction, values, authenticated=False):
    body = b"".join(attribute(key, value) for key, value in values)
    length = len(body) + (24 if authenticated else 0)
    header = struct.pack("!HHI12s", kind, length, COOKIE, transaction)
    if authenticated:
        body += attribute(0x0008, hmac.new(KEY, header + body, hashlib.sha1).digest())
    return header + body


class Relay(asyncio.DatagramProtocol):
    def __init__(self, server, client):
        self.server = server
        self.client = client
        self.channels = {}
        self.transport = None

    def connection_made(self, transport):
        self.transport = transport

    def datagram_received(self, data, peer):
        channel = next((key for key, value in self.channels.items() if value == peer), None)
        if channel is not None:
            packet = struct.pack("!HH", channel, len(data)) + data
        else:
            packet = message(0x0017, bytes(12), [(0x0012, xor_address(peer)), (0x0013, data)])
        self.server.transport.sendto(packet, self.client)


class Turn(asyncio.DatagramProtocol):
    def __init__(self, stun_enabled=True, on_binding=None):
        self.allocations = {}
        self.transport = None
        self.stun_enabled = stun_enabled
        self.on_binding = on_binding

    def connection_made(self, transport):
        self.transport = transport

    def datagram_received(self, data, client):
        asyncio.create_task(self.answer(data, client))

    async def allocate(self, client):
        if client not in self.allocations:
            relay = Relay(self, client)
            await asyncio.get_running_loop().create_datagram_endpoint(
                lambda: relay, local_addr=("198.18.0.1", 0)
            )
            self.allocations[client] = relay
        return self.allocations[client]

    async def answer(self, data, client):
        if len(data) < 4:
            return
        kind, length = struct.unpack_from("!HH", data)
        if 0x4000 <= kind <= 0x7FFF:
            allocation = self.allocations.get(client)
            if allocation and kind in allocation.channels:
                allocation.transport.sendto(data[4 : 4 + length], allocation.channels[kind])
            return
        if len(data) < 20 or struct.unpack_from("!I", data, 4)[0] != COOKIE:
            return
        values = attributes(data)
        transaction = data[8:20]
        if kind == 0x0001:
            if self.on_binding:
                self.on_binding(client)
            if not self.stun_enabled:
                return
            # The mapped tuple is the source actually observed after kernel
            # SNAT in the separate external-service namespace.
            reply = message(0x0101, transaction, [(0x0020, xor_address(client))])
        elif kind == 0x0003 and 0x0015 not in values:
            reply = message(0x0113, transaction, [
                (0x0009, bytes([0, 0, 4, 1]) + b"Unauthorized"),
                (0x0014, REALM), (0x0015, NONCE),
            ])
        elif kind == 0x0003:
            relay = await self.allocate(client)
            reply = message(0x0103, transaction, [
                (0x0016, xor_address(relay.transport.get_extra_info("sockname"))),
                (0x0020, xor_address(client)), (0x000D, struct.pack("!I", 600)),
            ], True)
        elif kind == 0x0004:
            reply = message(0x0104, transaction, [(0x000D, struct.pack("!I", 600))], True)
        elif kind == 0x0008:
            reply = message(0x0108, transaction, [], True)
        elif kind == 0x0009:
            relay = self.allocations[client]
            relay.channels[struct.unpack_from("!H", values[0x000C])[0]] = peer_address(values[0x0012])
            reply = message(0x0109, transaction, [], True)
        elif kind == 0x0016:
            self.allocations[client].transport.sendto(values[0x0013], peer_address(values[0x0012]))
            return
        else:
            return
        self.transport.sendto(reply, client)
