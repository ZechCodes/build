"""Initial cache controls for the #377 production-scout namespace fixture."""

import json
import subprocess
import time

BRIDGE = "10.72.0.1"
PHONE = "10.72.3.254"
CACHE_STATES = {"cached": "PERMANENT", "refresh": "REACHABLE", "stale": "STALE"}


def validate_initial(case, bridge, proxy, phone, phone_ip=PHONE):
    assert not any(row.get("dst") == phone_ip for row in bridge + proxy), "bridge already knows phone"
    cached = next((row for row in phone if row.get("dst") == BRIDGE), None)
    if case in CACHE_STATES:
        state = CACHE_STATES[case]
        assert cached and cached.get("lladdr") and state in cached.get("state", []), "cached MAC missing or wrong state"
    else:
        assert cached is None, "phone already knows bridge"


def initial(case, peer, artifacts, phone_ip=PHONE):
    def read(*args):
        return json.loads(subprocess.run(args, check=True, text=True, capture_output=True).stdout)

    bridge = read("ip", "-j", "neigh", "show", "dev", "eth0")
    proxy = read("ip", "-j", "neigh", "show", "proxy", "dev", "eth0")
    phone = read("nsenter", "-t", str(peer), "-n", "--", "ip", "-j", "neigh", "show", "dev", "eth0")
    validate_initial(case, bridge, proxy, phone, phone_ip)
    report = {"at": time.time(), "case": case, "phone_ip": phone_ip,
              "bridge": bridge, "proxy": proxy, "phone": phone}
    (artifacts / "arp-initial.json").write_text(json.dumps(report, indent=2))


def configure(case, peer):
    # Only the named investigation control blocks UDP9 before neighbour resolution.
    # Every permanent regression mode keeps the production scout path enabled.
    if case == "suppressed":
        suppress_scouts()
    if case not in CACHE_STATES:
        return
    interface = json.loads(subprocess.run(["ip", "-j", "link", "show", "eth0"],
                                         text=True, capture_output=True, check=True).stdout)[0]
    subprocess.run(["nsenter", "-t", str(peer), "-n", "--", "ip", "neigh", "add", BRIDGE,
                    "lladdr", interface["address"], "nud", CACHE_STATES[case].lower(), "dev", "eth0"], check=True)


def suppress_scouts():
    subprocess.run(["nft", "-f", "-"], input="""
table inet suppress_scouts {
 chain output { type filter hook output priority 0; policy accept;
   ip daddr 10.72.0.0/22 udp dport 9 counter drop
 }
}
""", text=True, check=True)
