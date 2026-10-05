#!/usr/bin/env python3
"""Run the #372/#374 fixture in private user/network namespaces."""

import os
import json
import re
import signal
from pathlib import Path
import subprocess
import sys
import time

from isolation import require_private_namespace
import arp_case

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


def sample_host_pressure(artifacts):
    advertised = artifacts / "bridge-host-socket.json"
    before = artifacts / "before.json"
    if advertised.exists():
        port = json.loads(advertised.read_text())["port"]
    elif before.exists():
        port = json.loads(before.read_text())["before"].get("hostSocketPort")
    else:
        return None
    if port is None:
        return None
    output = subprocess.run(["ss", "-u", "-a", "-m", "-n"], text=True,
                            capture_output=True, check=True).stdout.splitlines()
    for index, line in enumerate(output):
        if f"10.72.0.1:{port}" not in line.split() or index + 1 >= len(output):
            continue
        memory = re.search(r"skmem:\(r(\d+),rb\d+,t(\d+),tb(\d+)", output[index + 1])
        if not memory:
            continue
        neighbors = json.loads(subprocess.run(["ip", "-j", "neigh", "show", "dev", "eth0"],
                                             text=True, capture_output=True, check=True).stdout)
        states = {}
        for neighbor in neighbors:
            for state in neighbor.get("state", []):
                states[state] = states.get(state, 0) + 1
        # RTM_GETNEIGHTBL metadata includes the real global IPv4 ARP entry
        # count and allocation-failure counter across network namespaces.
        # Save only numeric aggregates, never interface names or addresses.
        tables = json.loads(subprocess.run(["ip", "-s", "-j", "ntable", "show", "name", "arp_cache"],
                                          text=True, capture_output=True, check=True).stdout)
        table = next(row for row in tables if "entries" in row and "thresh3" in row)
        return {"at": time.time(), "rx_occupied": int(memory[1]),
                "tx_occupied": int(memory[2]), "tx_capacity": int(memory[3]),
                "neighbor_states": states, "global_arp_entries": table["entries"],
                "global_gc_thresh2": table["thresh2"], "global_gc_thresh3": table["thresh3"],
                "global_table_fulls": table["table_fulls"]}
    return None


def inside(binary, artifacts):
    require_private_namespace()
    artifacts.mkdir(parents=True, exist_ok=True)
    environment = {"PATH": os.environ["PATH"], "HOME": str(artifacts / "home"),
                   "LANG": "C.UTF-8", "BRIDGE_IDENTITY_FILE": str(artifacts / "identity.json"),
                   "PYTHONDONTWRITEBYTECODE": "1",
                   "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null",
                   "BUILD_RTC_LAN_CHILD": "1", "BUILD_RTC_LAN_READY": str(artifacts / "bridge-ready"),
                   "BUILD_RTC_LAN_FINISHED": str(artifacts / "bridge-finished")}
    Path(environment["HOME"]).mkdir(exist_ok=True)
    if os.environ.get("BUILD_RTC_LAN_BASELINE"):
        environment["BUILD_RTC_LAN_BASELINE"] = "1"
    mode = os.environ.get("BUILD_RTC_LAN_MODE", "delayed")
    assert mode in {"delayed", "early-unresolved", "far-edge-unresolved", "far-edge-pressure", "unresolved", "late-unresolved", "large-subnet", "unknown-neighbor-unresolved", "unknown-neighbor-clustered", "unknown-neighbor-pressure", "different-nat", "missing-srflx", "arp-cold", "arp-cached", "arp-refresh", "arp-stale", "arp-proxy", "arp-active"}, f"unknown fixture mode: {mode}"
    environment["BUILD_RTC_LAN_MODE"] = mode
    if os.environ.get("BUILD_RTC_LAN_SWEEP_BASELINE"):
        environment["BUILD_RTC_LAN_SWEEP_BASELINE"] = "1"
    unknown = mode.startswith("unknown-neighbor-")
    arp = mode.startswith("arp-")
    wide = mode in {"far-edge-unresolved", "far-edge-pressure"} or unknown or arp
    prefix = 21 if mode == "large-subnet" else 22 if wide else 24
    phone = "10.72.1.2" if mode == "unknown-neighbor-clustered" else "10.72.3.254" if wide else "10.72.0.2"
    environment["BUILD_RTC_LAN_PHONE_IP"] = phone
    processes = []
    logs = []

    def start(name, arguments, child_environment=environment):
        log = open(artifacts / f"{name}.log", "w")
        logs.append(log)
        process = subprocess.Popen(arguments, env=child_environment, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        processes.append(process)
        return process

    def new_namespace(name):
        process = start(name, ["unshare", "-n", "sleep", "180"])
        current_namespace = os.readlink("/proc/self/ns/net")
        deadline = time.monotonic() + 3
        while os.readlink(f"/proc/{process.pid}/ns/net") == current_namespace:
            assert time.monotonic() < deadline
            time.sleep(0.01)
        return process.pid

    try:
        peer = new_namespace("namespace")
        run("ip", "link", "set", "lo", "up")
        gateway = new_namespace("router-namespace")
        service = new_namespace("service-namespace")
        run("ip", "link", "add", "eth0", "type", "veth", "peer", "name", "bridge0")
        run("ip", "link", "set", "bridge0", "netns", str(gateway))
        run("ip", "link", "add", "phone0", "type", "veth", "peer", "name", "browser0")
        run("ip", "link", "set", "phone0", "netns", str(gateway))
        run("ip", "link", "set", "browser0", "netns", str(peer))
        run(*namespace_command(gateway, "ip", "link", "add", "wan0", "type", "veth", "peer", "name", "service0"))
        run(*namespace_command(gateway, "ip", "link", "set", "service0", "netns", str(service)))
        for arguments in [
            ["link", "set", "lo", "up"],
            ["link", "add", "lan", "type", "bridge"],
            ["addr", "add", f"10.72.0.254/{prefix}", "dev", "lan"],
            ["addr", "add", "198.18.0.2/32", "dev", "lo"],
            ["addr", "add", "203.0.113.1/32", "dev", "lo"],
            ["addr", "add", "203.0.113.2/32", "dev", "lo"],
            ["addr", "add", "198.18.0.254/24", "dev", "wan0"],
            ["link", "set", "bridge0", "master", "lan"],
            ["link", "set", "phone0", "master", "lan"],
            ["link", "set", "bridge0", "up"],
            ["link", "set", "phone0", "up"],
            ["link", "set", "lan", "up"],
            ["link", "set", "wan0", "up"],
        ]:
            run(*namespace_command(gateway, "ip", *arguments))
        # Same-interface forwarding preserves the authentic host-check tuple.
        # No redirects, proxy ARP or cache mutation may warm an unknown phone.
        run(*namespace_command(gateway, "sysctl", "-q", "-w", "net.ipv4.ip_forward=1",
                              "net.ipv4.conf.all.send_redirects=0", "net.ipv4.conf.lan.send_redirects=0",
                              "net.ipv4.conf.all.proxy_arp=0", "net.ipv4.conf.lan.proxy_arp=0"))
        phone_public = "203.0.113.2" if mode == "different-nat" else "203.0.113.1"
        run(*namespace_command(gateway, "nft", "-f", "-"), input=f"""
table ip service_nat {{
 chain postrouting {{ type nat hook postrouting priority 100; policy accept;
   ip saddr 10.72.0.1 ip daddr 198.18.0.1 ip protocol udp snat to 203.0.113.1
   ip saddr {phone} ip daddr 198.18.0.1 ip protocol udp snat to {phone_public}
 }}
}}
""")
        for arguments in [
            ["link", "set", "lo", "up"],
            ["link", "set", "service0", "name", "eth0"],
            ["addr", "add", "198.18.0.1/24", "dev", "eth0"],
            ["link", "set", "eth0", "up"],
            ["route", "add", "default", "via", "198.18.0.254"],
        ]:
            run(*namespace_command(service, "ip", *arguments))
        run("ip", "addr", "add", f"10.72.0.1/{prefix}", "dev", "eth0")
        run("ip", "link", "set", "eth0", "up")
        # Enable conntrack before opening signaling, then tighten the policy.
        run("nft", "-f", "-", input="""
table inet bridge_firewall {
 chain input { type filter hook input priority 0; policy accept;
   ct state established,related accept
 }
}
""")
        run(*namespace_command(peer, "ip", "link", "set", "browser0", "name", "eth0"))
        run(*namespace_command(peer, "ip", "addr", "add", f"{phone}/{prefix}", "dev", "eth0"))
        run(*namespace_command(peer, "ip", "link", "set", "lo", "up"))
        run(*namespace_command(peer, "ip", "link", "set", "eth0", "up"))
        run(*namespace_command(peer, "ip", "route", "add", "default", "via", "10.72.0.254"))
        if unknown or mode == "arp-proxy":
            run(*namespace_command(peer, "ip", "route", "add", "10.72.0.1/32", "via", "10.72.0.254"))
            run(*namespace_command(peer, "sysctl", "-q", "-w", "net.ipv4.conf.all.accept_redirects=0",
                                  "net.ipv4.conf.eth0.accept_redirects=0"))
        for address in ["198.18.0.1/32", "198.18.0.2/32"]:
            run("ip", "route", "add", address, "via", "10.72.0.254")
        if arp:
            arp_case.configure(mode.removeprefix("arp-"), peer)
            for side, command in [("bridge", []), ("phone", namespace_command(peer))]:
                observer = start(f"{side}-observe", [*command, "python3", str(HERE / "arp_observe.py"), str(artifacts), side])
                wait_file(artifacts / f"{side}-observe-ready", observer)
        router = start("router", namespace_command(gateway, "python3", str(HERE / "router.py"), str(artifacts)))
        wait_file(artifacts / "router-ready", router)
        external = start("service", namespace_command(service, "python3", str(HERE / "service.py"), str(artifacts)))
        wait_file(artifacts / "service-ready", external)
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
        bridge = start("bridge", ["env", "-i", *(f"{key}={value}" for key, value in environment.items()), str(binary), "late_mdns_host_is_checked_before_the_spa_restarts_ice", "--ignored", "--exact", "--nocapture"])
        wait_file(artifacts / "bridge-ready", bridge)
        check = "arp-check.mjs" if arp else "check.mjs"
        browser = start("browser", namespace_command(peer, "node", str(HERE / check), str(artifacts)))
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
        if arp:
            arp_case.initial(mode.removeprefix("arp-"), peer, artifacts)
        (artifacts / "firewall-ready").write_text("ready")
        if unknown:
            wait_file(artifacts / "phone-gated", network)
            neighbors = json.loads(subprocess.run(["ip", "-j", "neigh", "show", "dev", "eth0"],
                                                 text=True, capture_output=True, check=True).stdout)
            proxy = json.loads(subprocess.run(["ip", "-j", "neigh", "show", "proxy", "dev", "eth0"],
                                             text=True, capture_output=True, check=True).stdout)
            absent = not any(row.get("dst") == phone for row in neighbors + proxy)
            (artifacts / "neighbor-before.json").write_text(json.dumps({"at": time.time(), "phone_absent": absent,
                                                                       "proxy_entries": len(proxy)}))
            assert absent, "the far-edge phone must be genuinely absent before authentic host trickle"
            (artifacts / "gated").write_text("ready")
        if arp:
            wait_file(artifacts / "phone-gated", network)
            (artifacts / "gated").write_text("ready")
        if mode == "far-edge-pressure" or unknown or arp:
            samples = []
            deadline = time.monotonic() + 65
            while browser.poll() is None:
                if time.monotonic() >= deadline:
                    raise subprocess.TimeoutExpired(browser.args, 65)
                sample = sample_host_pressure(artifacts)
                if sample is not None:
                    samples.append(sample)
                    temporary = artifacts / "pressure.json.tmp"
                    temporary.write_text(json.dumps(samples))
                    temporary.replace(artifacts / "pressure.json")
                time.sleep(0.25)
            result = browser.returncode
        else:
            result = browser.wait(timeout=65)
        if result == 0:
            wait_file(artifacts / "bridge-finished", bridge, seconds=3)
        rules = subprocess.run(["nft", "-j", "list", "ruleset"], text=True, capture_output=True, check=True)
        (artifacts / "firewall.json").write_text(rules.stdout)
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
        for name in ["browser", "network", "bridge", "router", "service"]:
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
