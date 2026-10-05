"""Actual off-link STUN/TURN reached through the disposable gateway's SNAT."""

import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from isolation import require_private_namespace
from turn import Turn


async def main(artifacts):
    require_private_namespace()
    observed = []

    def record_binding(client):
        # Scoped packet evidence, never a candidate source or product log.
        observed.append({"at": time.time(), "ip": client[0], "port": client[1]})
        temporary = artifacts / "stun-observed.json.tmp"
        temporary.write_text(json.dumps(observed))
        temporary.replace(artifacts / "stun-observed.json")

    await asyncio.get_running_loop().create_datagram_endpoint(
        lambda: Turn(stun_enabled=False),
        local_addr=("198.18.0.1", 3478),
    )
    # Separate STUN's tuple from TURN's transport demultiplexer. Both still
    # observe the genuine source selected by destination-scoped kernel SNAT.
    await asyncio.get_running_loop().create_datagram_endpoint(
        lambda: Turn(stun_enabled=os.environ.get("BUILD_RTC_LAN_MODE") != "missing-srflx", on_binding=record_binding),
        local_addr=("198.18.0.1", 3479),
    )
    (artifacts / "service-ready").write_text("ready")
    while not (artifacts / "disconnect-host").exists():
        await asyncio.sleep(0.02)
    # History-only failure phase cuts every external UDP return, along with
    # the phone's direct checks, so real ICE reaches its default Failed state.
    subprocess.run(["nft", "-f", "-"], input="""
table inet fail_bridge_ice {
 chain output { type filter hook output priority 0; policy accept;
   ip protocol udp drop
 }
}
""", text=True, check=True)
    (artifacts / "service-host-disconnected").write_text("ready")
    while True:
        await asyncio.sleep(0.1)


if __name__ == "__main__":
    asyncio.run(main(Path(sys.argv[1])))
