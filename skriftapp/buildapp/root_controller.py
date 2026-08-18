"""Root: the marketing landing page.

GET / serves the static landing page; its CTA links into the SPA at /app/,
which owns the auth gate. Landing assets live in ``buildapp/landing/`` — not in
``buildapp/static/``, which the SPA build wipes (emptyOutDir) — and are served
from /landing/* with the same traversal guard as the SPA assets. The page is
CSP-clean: inline styles only (style-src allows them), scripts as same-origin
files.
"""

import asyncio
from pathlib import Path

from litestar import Controller, get
from litestar.exceptions import NotFoundException
from litestar.response import Response

LANDING_DIR = Path(__file__).parent / "landing"

LANDING_MEDIA_TYPES = {
    ".js": "text/javascript",
    ".css": "text/css",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".ico": "image/x-icon",
    ".woff2": "font/woff2",
}


class RootController(Controller):
    path = ""

    @get("/")
    async def root(self) -> Response:
        html = await asyncio.to_thread((LANDING_DIR / "index.html").read_text)
        return Response(html, media_type="text/html")

    # Sync on purpose: Litestar runs it in its threadpool (sync_to_thread=True),
    # keeping file reads off the event loop — same shape as the SPA asset route.
    @get("/landing/{asset_path:path}", sync_to_thread=True)
    def landing_asset(self, asset_path: str) -> Response:
        resolved = (LANDING_DIR / asset_path.lstrip("/")).resolve()
        if not resolved.is_relative_to(LANDING_DIR.resolve()) or not resolved.is_file():
            raise NotFoundException()
        media_type = LANDING_MEDIA_TYPES.get(resolved.suffix, "application/octet-stream")
        return Response(resolved.read_bytes(), media_type=media_type)
