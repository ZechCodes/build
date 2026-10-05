"""Off-link TURN and transparent rendezvous for the unknown-neighbor fixture."""

import asyncio
from pathlib import Path
import sys

from isolation import require_private_namespace
from turn import Turn


async def rendezvous(reader, writer):
    # The bridge sees only this router's L2 source. Encrypted frames and native
    # signaling contents are copied unchanged; no ICE candidates are invented.
    bridge_reader, bridge_writer = await asyncio.open_connection("10.72.0.1", 9000)

    async def copy(source, destination):
        try:
            while packet := await source.read(65536):
                destination.write(packet)
                await destination.drain()
        finally:
            destination.close()

    await asyncio.gather(copy(reader, bridge_writer), copy(bridge_reader, writer))


async def main(artifacts):
    require_private_namespace()
    loop = asyncio.get_running_loop()
    await loop.create_datagram_endpoint(Turn, local_addr=("198.18.0.1", 3478))
    server = await asyncio.start_server(rendezvous, "198.18.0.2", 9000)
    (artifacts / "router-ready").write_text("ready")
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main(Path(sys.argv[1])))
