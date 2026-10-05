#!/usr/bin/env python3
"""Run the vendor socket-pressure test entirely in disposable namespaces."""

import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

from isolation import require_private_namespace


TEST = "runtime::host_egress::tests::blocked_neighbors_leave_ordinary_ice_and_maximum_gso_send_immediately_ready"


def run(*arguments):
    subprocess.run(arguments, check=True, timeout=5)


def inside(binary):
    require_private_namespace()
    parent = os.environ["BUILD_RTC_PRESSURE_PARENT_NET_NS"]
    if os.readlink("/proc/self/ns/net") == parent:
        raise AssertionError("socket-pressure fixture requires a fresh network namespace")
    processes = []
    try:
        peer = subprocess.Popen(["unshare", "-n", "sleep", "30"], start_new_session=True)
        processes.append(peer)
        own = os.readlink("/proc/self/ns/net")
        deadline = time.monotonic() + 3
        while os.readlink(f"/proc/{peer.pid}/ns/net") == own:
            if time.monotonic() >= deadline:
                raise AssertionError("peer did not enter its private network namespace")
            time.sleep(0.01)
        run("ip", "link", "set", "lo", "up")
        run("ip", "link", "add", "eth0", "type", "veth", "peer", "name", "peer0")
        run("ip", "link", "set", "peer0", "netns", str(peer.pid))
        run("ip", "addr", "add", "10.72.0.1/22", "dev", "eth0")
        run("ip", "link", "set", "eth0", "up")
        prefix = ["nsenter", "-t", str(peer.pid), "-n", "--preserve-credentials", "--"]
        run(*prefix, "ip", "link", "set", "peer0", "name", "eth0")
        run(*prefix, "ip", "addr", "add", "10.72.0.2/22", "dev", "eth0")
        run(*prefix, "ip", "link", "set", "eth0", "up")
        with tempfile.TemporaryDirectory(prefix="rtc374-pressure-peer-") as temporary:
            ready = Path(temporary) / "ready"
            server = subprocess.Popen([*prefix, sys.executable, "-c", """
import socket, sys
from pathlib import Path
receiver = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
receiver.bind(('10.72.0.2', 40000))
Path(sys.argv[1]).write_text('ready')
while True:
    receiver.recvfrom(65535)
""", str(ready)], start_new_session=True)
            processes.append(server)
            deadline = time.monotonic() + 3
            while not ready.exists():
                if server.poll() is not None or time.monotonic() >= deadline:
                    raise AssertionError("private UDP receiver did not start")
                time.sleep(0.01)
            return subprocess.run([str(binary), TEST, "--ignored", "--exact", "--nocapture"],
                                  timeout=15).returncode
    finally:
        for process in reversed(processes):
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        for process in processes:
            process.wait(timeout=3)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--inside":
        sys.exit(inside(Path(sys.argv[2]).resolve()))
    binary = Path(sys.argv[1]).resolve()
    environment = os.environ.copy()
    environment["BUILD_RTC_PRESSURE_PARENT_NET_NS"] = os.readlink("/proc/self/ns/net")
    sys.exit(subprocess.run(["unshare", "-Urn", sys.executable, str(Path(__file__).resolve()),
                            "--inside", str(binary)], env=environment, timeout=25).returncode)
