#!/usr/bin/env python3
"""Run the task #372 fixture in private user/network namespaces."""

import os
import signal
from pathlib import Path
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent


def run(*arguments, input=None):
    return subprocess.run(arguments, input=input, text=True, check=True)


def wait_file(path, process, seconds=15):
    deadline = time.monotonic() + seconds
    while not path.exists():
        assert process.poll() is None, f"fixture exited {process.returncode} before {path.name}"
        assert time.monotonic() < deadline, f"fixture did not create {path.name} within {seconds}s"
        time.sleep(0.02)


def namespace_command(pid, *arguments):
    return ["nsenter", "-t", str(pid), "-n", "--", *arguments]


def inside(binary, artifacts):
    mapping = Path("/proc/self/uid_map").read_text().split()
    assert os.geteuid() == 0 and mapping[0] == "0" and mapping[2] == "1", \
        "firewall fixture requires its disposable single-user namespace"
    artifacts.mkdir(parents=True, exist_ok=True)
    environment = {"PATH": os.environ["PATH"], "HOME": str(artifacts / "home"),
                   "LANG": "C.UTF-8", "BRIDGE_IDENTITY_FILE": str(artifacts / "identity.json"),
                   "PYTHONDONTWRITEBYTECODE": "1",
                   "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null",
                   "BUILD_RTC_LAN_CHILD": "1", "BUILD_RTC_LAN_READY": str(artifacts / "bridge-ready")}
    Path(environment["HOME"]).mkdir(exist_ok=True)
    if os.environ.get("BUILD_RTC_LAN_BASELINE"):
        environment["BUILD_RTC_LAN_BASELINE"] = "1"
    processes = []
    logs = []

    def start(name, arguments, child_environment=environment):
        log = open(artifacts / f"{name}.log", "w")
        logs.append(log)
        process = subprocess.Popen(arguments, env=child_environment, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        processes.append(process)
        return process

    try:
        browser_namespace = start("namespace", ["unshare", "-n", "sleep", "180"])
        # Ensure unshare has entered its new network namespace before moving the peer.
        current_namespace = os.readlink("/proc/self/ns/net")
        deadline = time.monotonic() + 3
        while os.readlink(f"/proc/{browser_namespace.pid}/ns/net") == current_namespace:
            assert time.monotonic() < deadline
            time.sleep(0.01)
        peer = browser_namespace.pid
        run("ip", "link", "set", "lo", "up")
        run("ip", "link", "add", "eth0", "type", "veth", "peer", "name", "browser0")
        run("ip", "link", "set", "browser0", "netns", str(peer))
        run("ip", "addr", "add", "10.72.0.1/24", "dev", "eth0")
        run("ip", "link", "set", "eth0", "up")
        # Enable conntrack before opening signaling, then tighten the policy
        # after that TCP connection is established.
        run("nft", "-f", "-", input="""
table inet bridge_firewall {
 chain input { type filter hook input priority 0; policy accept;
   ct state established,related accept
 }
}
""")
        run(*namespace_command(peer, "ip", "link", "set", "browser0", "name", "eth0"))
        run(*namespace_command(peer, "ip", "addr", "add", "10.72.0.2/24", "dev", "eth0"))
        run(*namespace_command(peer, "ip", "link", "set", "lo", "up"))
        run(*namespace_command(peer, "ip", "link", "set", "eth0", "up"))
        run(*namespace_command(peer, "ip", "route", "add", "default", "via", "10.72.0.1"))
        run(*namespace_command(peer, "nft", "-f", "-"), input="""
table inet delay_mdns {
 chain output { type filter hook output priority 0; policy accept;
 }
}
table inet hold_checks {
 chain output { type filter hook output priority 0; policy accept;
 }
}
""")
        network = start("network", namespace_command(peer, "python3", str(HERE / "network.py"), str(artifacts)))
        wait_file(artifacts / "network-ready", network)
        bridge = start("bridge", [str(binary), "late_mdns_host_is_checked_before_the_spa_restarts_ice", "--ignored", "--exact", "--nocapture"])
        wait_file(artifacts / "bridge-ready", bridge)
        browser = start("browser", namespace_command(peer, "node", str(HERE / "check.mjs"), str(artifacts)))
        wait_file(artifacts / "browser-ready", browser)
        # Signaling's TCP connection predates the firewall, so the only inbound
        # allowances here are stock UFW's established traffic and mDNS.
        run("nft", "delete", "table", "inet", "bridge_firewall")
        run("nft", "-f", "-", input="""
table inet bridge_firewall {
 chain input { type filter hook input priority 0; policy drop;
   iifname "lo" accept
   ct state established,related accept
   ip daddr 224.0.0.251 udp dport 5353 accept
   counter drop
 }
}
""")
        (artifacts / "firewall-ready").write_text("ready")
        result = browser.wait(timeout=65)
        run("nft", "-j", "list", "ruleset")
        assert result == 0, f"Chromium check exit {result}; artifacts {artifacts}"
    finally:
        for process in reversed(processes):
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        for process in processes:
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for log in logs:
            log.close()
        for name in ["browser", "network", "bridge"]:
            log_path = artifacts / (name + ".log")
            if log_path.exists():
                print(f"{name}:\n{log_path.read_text()}")
        if (artifacts / "wire.json").exists():
            print("wire:", (artifacts / "wire.json").read_text())


if __name__ == "__main__":
    if sys.argv[1] == "--inside":
        inside(Path(sys.argv[2]), Path(sys.argv[3]))
    else:
        binary, temporary_directory = sys.argv[1:3]
        artifacts = Path(os.environ.get("BUILD_RTC_LAN_ARTIFACTS", temporary_directory))
        result = subprocess.run(["unshare", "-Urn", "python3", str(HERE / "run.py"),
                                 "--inside", binary, str(artifacts)])
        sys.exit(result.returncode)
