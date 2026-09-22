#!/usr/bin/env python3
"""Preview the public landing routes without starting agents or requiring login.

Run with skriftapp/.venv/bin/python scripts/preview-landing.py. The homepage is the
Astro build, so build it first: `cd landing && npm run build`. Without it, `/`
answers 503 and says so — the preview serves what is on disk and never renders a
stand-in page.

The real POST /api/waitlist lives in buildapp.waitlist_controller, which needs a
database session, Skrift settings, and an email backend; none of those exist in this
loopback app, so it is NOT mounted here. `--waitlist-stub` mounts a handler that
answers 204 and stores nothing, which is enough to drive the form's success path in
a browser. Account/download actions still belong to the complete application; this
preview never creates sessions, mints tokens, or bypasses the alpha guard.
"""

import argparse
import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "skriftapp"))

from hypercorn.asyncio import serve  # noqa: E402
from hypercorn.config import Config  # noqa: E402
from litestar import Litestar, post  # noqa: E402
from litestar.status_codes import HTTP_204_NO_CONTENT  # noqa: E402

from buildapp.root_controller import RootController  # noqa: E402
from buildapp.waitlist_controller import JOIN_ROUTE_PATH  # noqa: E402

CONTENT_SECURITY_POLICY = (
    "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; "
    "style-src 'self' 'unsafe-inline'; img-src 'self' data:; "
    "font-src 'self'; connect-src 'self' data:; worker-src 'self' blob:; "
    "frame-ancestors 'none'; base-uri 'self'; object-src 'none'"
)


@post(JOIN_ROUTE_PATH, status_code=HTTP_204_NO_CONTENT, sync_to_thread=False)
def waitlist_stub() -> None:
    """Accept a signup and drop it. The form's client only reads `response.ok`, so
    this exercises the success path without a database or a mail backend."""
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=4173)
    parser.add_argument(
        "--waitlist-stub",
        action="store_true",
        help="answer POST /api/waitlist with 204 and store nothing",
    )
    args = parser.parse_args()
    config = Config()
    config.bind = [f"127.0.0.1:{args.port}"]
    handlers = [RootController, waitlist_stub] if args.waitlist_stub else [RootController]
    app = Litestar(
        route_handlers=handlers, debug=False,
        response_headers={"Content-Security-Policy": CONTENT_SECURITY_POLICY},
    )
    print(f"Landing preview: http://127.0.0.1:{args.port}", flush=True)
    if args.waitlist_stub:
        print(
            f"The preview stubs {JOIN_ROUTE_PATH}: it answers 204 and stores nothing.",
            flush=True,
        )
    asyncio.run(serve(app, config))


if __name__ == "__main__":
    main()
