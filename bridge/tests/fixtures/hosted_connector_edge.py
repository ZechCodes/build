"""Loopback-only edge and HTTP origin for the real hosted connector test.

Run with the Rust test, which starts and supervises the pinned connector.
The mock performs no tunnel registration and uses no Cloudflare account.
"""

import http.server
import json
import os
from pathlib import Path
import socket
import ssl
import subprocess
import threading
import time
import sys

import h2.connection
import h2.events

host = 'b-4071dc1c1a3d412b8a47cf8bfaf0bf51.hosted.example'
websocket_nonce = 'dGhlIHNhbXBsZSBub25jZQ=='

class Origin(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def do_GET(self):
        assert self.headers['Host'] == host, self.headers['Host']
        match self.path:
            case '/api':
                body = b'{"origin":"bridge"}'
                self.send_response(200)
                self.send_header('content-type', 'application/json')
                self.send_header('content-length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            case '/events':
                self.send_response(200)
                self.send_header('content-type', 'text/event-stream')
                self.send_header('connection', 'close')
                self.end_headers()
                self.wfile.write(b'data: first\n\n')
                self.wfile.flush()
                assert self.server.release_events.wait(5)
                self.wfile.write(b'data: second\n\n')
            case '/terminal':
                assert self.headers['Upgrade'] == 'websocket'
                assert self.headers['Sec-WebSocket-Key'] == websocket_nonce
                self.send_response(101)
                self.send_header('Connection', 'Upgrade')
                self.send_header('Upgrade', 'websocket')
                # RFC 6455's fixed response for the public sample challenge above.
                self.send_header('Sec-WebSocket-Accept', 's3pPLMBiTxaQ9kYGzzhZRbK+xOo=')
                self.end_headers()
                request = self.rfile.read(8)
                assert request == b'\x81\x82\x01\x02\x03\x04qk', request
                self.wfile.write(b'\x81\x04pong')
                self.close_connection = True
            case _:
                self.send_error(500)

    def log_message(self, *_):
        pass

class Edge:
    def __init__(self, socket):
        self.socket = socket
        self.h2 = h2.connection.H2Connection()
        self.h2.initiate_connection()
        self.flush()
        self.events = []

    def flush(self):
        self.socket.sendall(self.h2.data_to_send())

    def request(self, path, authority=host, websocket=False):
        stream = self.h2.get_next_available_stream_id()
        headers = [
            (':method', 'GET'), (':scheme', 'https'), (':path', path),
            (':authority', authority)
        ]
        if websocket:
            headers += [
                ('cf-cloudflared-proxy-connection-upgrade', 'websocket'),
                ('sec-websocket-key', websocket_nonce),
            ]
        self.h2.send_headers(stream, headers, end_stream=not websocket)
        self.flush()
        return stream

    def wait(self, stream, kind):
        while True:
            for i, event in enumerate(self.events):
                if isinstance(event, kind) and event.stream_id == stream:
                    return self.events.pop(i)
            data = self.socket.recv(65536)
            assert data, 'connector closed its edge connection'
            for event in self.h2.receive_data(data):
                if isinstance(event, h2.events.DataReceived):
                    self.h2.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
                self.events.append(event)
            self.flush()

    def status(self, stream):
        return dict(self.wait(stream, h2.events.ResponseReceived).headers)[b':status']


def certificate(directory, name, host):
    cert, key = directory / f"{name}.pem", directory / f"{name}.key"
    subprocess.run([
        "openssl", "req", "-new", "-newkey", "rsa:2048", "-x509", "-nodes",
        "-days", "1", "-subj", f"/CN={host}", "-addext", f"subjectAltName=DNS:{host}",
        "-out", cert, "-keyout", key,
    ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    return context, cert


def origin(directory):
    context, origin_ca = certificate(directory, "origin", host)

    def require_sni(_socket, server_name, _context):
        if server_name != host:
            raise ValueError("unexpected origin SNI")

    context.set_servername_callback(require_sni)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    server.release_events = threading.Event()
    server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, origin_ca


def forwarding(edge, server):
    rest = edge.request("/api")
    assert edge.status(rest) == b"200"
    assert edge.wait(rest, h2.events.DataReceived).data == b'{"origin":"bridge"}'
    print("REST reaches the TLS origin with exact Host and SNI", flush=True)
    sse = edge.request("/events")
    assert edge.status(sse) == b"200"
    assert edge.wait(sse, h2.events.DataReceived).data == b"data: first\n\n"
    server.release_events.set()
    assert edge.wait(sse, h2.events.DataReceived).data == b"data: second\n\n"
    print("SSE flushes the first event before the second is produced", flush=True)
    ws = edge.request("/terminal", websocket=True)
    assert edge.status(ws) == b"200"  # The connector maps origin 101 to 200 on HTTP/2.
    edge.h2.send_data(ws, b"\x81\x82\x01\x02\x03\x04qk")
    edge.flush()
    assert edge.wait(ws, h2.events.DataReceived).data == b"\x81\x04pong"
    print("WebSocket upgrade and bidirectional bytes reach the TLS origin", flush=True)
    assert edge.status(edge.request("/api", "wrong.hosted.example")) == b"404"
    print("An unrelated hostname hits the final 404", flush=True)


def push_configuration(edge, trap):
    stream = edge.h2.get_next_available_stream_id()
    edge.h2.send_headers(stream, [
        (":method", "POST"), (":scheme", "https"), (":path", "/config"),
        (":authority", host),
        ("cf-cloudflared-proxy-connection-upgrade", "update-configuration"),
    ])
    edge.h2.send_data(stream, json.dumps({
        "version": 0,
        "config": {"ingress": [{
            "hostname": host, "service": f"http://127.0.0.1:{trap.getsockname()[1]}",
        }, {"service": "http_status:404"}]},
    }).encode(), end_stream=True)
    pushed = time.monotonic()
    edge.flush()
    # The connector may die before finishing the update response. Read
    # through any buffered frames and require the transport to close.
    try:
        while edge.socket.recv(65536):
            pass
    except (ConnectionResetError, ssl.SSLEOFError):
        pass
    try:
        edge.request("/api")
        assert not edge.socket.recv(65536), "the retired edge served an assigned-host request"
    except (BrokenPipeError, ConnectionResetError, ssl.SSLEOFError):
        pass
    elapsed_ms = (time.monotonic() - pushed) * 1000
    print(f"Production monitor closed the edge transport {elapsed_ms:.1f} ms after the test push", flush=True)

    trap.settimeout(0.25)
    try:
        connection, _ = trap.accept()
    except TimeoutError:
        print("Replacement origin received zero connections", flush=True)
    else:
        connection.close()
        raise AssertionError("connector dialled the replacement origin")


def main(directory):
    assert {name for _, name in socket.if_nameindex()} == {"lo"}, "run in a loopback-only namespace"
    os.umask(0o077)
    edge_context, edge_ca = certificate(directory, "edge", "h2.cftunnel.com")
    server, ca = origin(directory)
    with socket.socket() as listener, socket.socket() as trap:
        for sock in (listener, trap):
            sock.bind(("127.0.0.1", 0))
            sock.listen()
            sock.settimeout(10)
        temporary = directory / "fixture.tmp"
        temporary.write_text(json.dumps({
            "edge": f"127.0.0.1:{listener.getsockname()[1]}",
            "edge_ca": str(edge_ca), "origin_ca": str(ca),
            "origin": f"127.0.0.1:{server.server_port}",
        }))
        temporary.rename(directory / "fixture.json")
        transport, _ = listener.accept()
        transport.settimeout(5)
        with edge_context.wrap_socket(transport, server_side=True) as client:
            edge = Edge(client)
            forwarding(edge, server)
            push_configuration(edge, trap)
    server.shutdown()
    server.server_close()


if __name__ == "__main__":
    main(Path(sys.argv[1]))
