"""Build app controller — serves the SPA bundle (built from spa/) behind Skrift auth.

The bundle is produced by ``cd spa && npm run build`` into ``buildapp/static/``:
``index.html`` (served at /app/ with the user's initial substituted) plus hashed
assets under ``static/assets/`` (js, css, self-hosted fonts). Zero CDN.
"""

import asyncio
from pathlib import Path

from litestar import Controller, Request, get
from litestar.exceptions import NotFoundException
from litestar.response import Redirect, Response
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from skrift.db.models.user import User

from buildapp.session_auth import session_user_id

HERE = Path(__file__).parent
STATIC_DIR = HERE / "static"

MEDIA_TYPES = {
    ".html": "text/html",
    ".js": "text/javascript",
    ".mjs": "text/javascript",
    ".css": "text/css",
    ".map": "application/json",
    ".json": "application/json",
    ".woff": "font/woff",
    ".woff2": "font/woff2",
    ".wasm": "application/wasm",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".ico": "image/x-icon",
}


class BuildController(Controller):
    path = "/app"

    @get("/")
    async def index(self, request: Request, db_session: AsyncSession) -> Response | Redirect:
        # Skrift auth via the shared session helper (a malformed session value
        # means "not logged in", never a 500). Unlike the API routes' auth_guard
        # (which answers 401), the SPA shell redirects to login — landing back on
        # the board afterwards, never the empty CMS root.
        user_id = session_user_id(request)
        if user_id is None:
            return Redirect("/auth/login?next=/app/")

        result = await db_session.execute(select(User).where(User.id == user_id))
        user = result.scalar_one_or_none()
        email = getattr(user, "email", None) or "user"

        html = await asyncio.to_thread((STATIC_DIR / "index.html").read_text)
        html = html.replace("{{USER0}}", email[0:1].upper())
        return Response(html, media_type="text/html")

    # Sync handlers on purpose: Litestar runs them in its threadpool
    # (sync_to_thread=True), keeping blocking file reads — including multi-MB
    # wasm assets — off the event loop.
    @get("/sw.js", sync_to_thread=True)
    def service_worker(self) -> Response:
        """The push service worker, at the root of the /app/ scope so it can
        control the SPA pages. Never cached immutably — a stale worker would
        outlive deploys. The worker itself is cache-free (no fetch handler)."""
        worker_path = STATIC_DIR / "sw.js"
        if not worker_path.is_file():
            raise NotFoundException()
        return Response(
            worker_path.read_bytes(),
            media_type="text/javascript",
            headers={"Cache-Control": "no-cache"},
        )

    @get("/static/{asset_path:path}", sync_to_thread=True)
    def static_asset(self, asset_path: str) -> Response:
        """Hashed bundle assets, same-origin so the strict CSP allows them."""
        resolved = (STATIC_DIR / asset_path.lstrip("/")).resolve()
        if not resolved.is_relative_to(STATIC_DIR.resolve()) or not resolved.is_file():
            raise NotFoundException()
        media_type = MEDIA_TYPES.get(resolved.suffix, "application/octet-stream")
        headers = {}
        if resolved.parent.name == "assets":
            # Content-hashed filenames never change content — cache forever.
            headers["Cache-Control"] = "public, max-age=31536000, immutable"
        return Response(resolved.read_bytes(), media_type=media_type, headers=headers)
