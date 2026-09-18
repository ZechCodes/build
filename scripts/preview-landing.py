#!/usr/bin/env python3
"""Preview the public landing routes without starting agents or requiring login.

Run with skriftapp/.venv/bin/python scripts/preview-landing.py. Account/download
actions still belong to the complete application; this loopback preview never
creates sessions, mints tokens, or bypasses the application's alpha guard.
"""

import argparse
import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "skriftapp"))

from hypercorn.asyncio import serve  # noqa: E402
from hypercorn.config import Config  # noqa: E402
from litestar import Litestar  # noqa: E402

from buildapp.root_controller import RootController  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4173)
    args = parser.parse_args()
    config = Config()
    config.bind = [f"127.0.0.1:{args.port}"]
    app = Litestar(
        route_handlers=[RootController], debug=False,
        response_headers={
            "Content-Security-Policy": (
                "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; "
                "style-src 'self' 'unsafe-inline'; img-src 'self' data:; "
                "font-src 'self'; connect-src 'self' data:; worker-src 'self' blob:; "
                "frame-ancestors 'none'; base-uri 'self'; object-src 'none'"
            ),
        },
    )
    print(f"Landing preview: http://127.0.0.1:{args.port}", flush=True)
    asyncio.run(serve(app, config))


if __name__ == "__main__":
    main()
