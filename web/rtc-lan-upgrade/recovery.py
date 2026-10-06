"""Counted native ICE loss confined to the fixture's disposable namespace."""

import json
from pathlib import Path
import subprocess
import sys
import time

from isolation import require_private_namespace


def configure(phase, artifacts):
    require_private_namespace()
    if phase == "release":
        rules = subprocess.run(["nft", "-j", "list", "table", "inet", "recovery_direct"],
                               text=True, capture_output=True, check=True)
        (artifacts / "recovery-firewall.json").write_text(rules.stdout)
        subprocess.run(["nft", "delete", "table", "inet", "recovery_direct"], check=True)
        (artifacts / "recovery-released.json").write_text(json.dumps({"at": time.time() * 1000}))
        return
    assert phase in {"fail", "recover"}, "unknown native recovery loss phase"
    if phase == "recover":
        subprocess.run(["nft", "delete", "table", "inet", "recovery_direct"], check=True)
    hook, address, kind = ("output", "daddr", "0x0001") if phase == "fail" else ("input", "saddr", "0x0101")
    subprocess.run(["nft", "add", "table", "inet", "recovery_direct"], check=True)
    subprocess.run(["nft", "add", "chain", "inet", "recovery_direct", hook,
                    f"{{ type filter hook {hook} priority -10; policy accept; }}"], check=True)
    # Match a STUN Binding request/success at UDP payload offsets. SCTP and
    # the bridge's consent request/phone response continue to cross normally.
    subprocess.run(["nft", "add", "rule", "inet", "recovery_direct", hook, "ip", address,
                    "10.72.0.1", "ip", "protocol", "udp", "@th,64,16", kind,
                    "@th,96,32", "0x2112a442", "counter", "drop"], check=True)
    (artifacts / f"recovery-{phase}-blocked.json").write_text(json.dumps({"at": time.time() * 1000}))


if __name__ == "__main__":
    configure(sys.argv[1], Path(sys.argv[2]))
