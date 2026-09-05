"""Build app controller — serves the SPA bundle (built from spa/) behind Skrift auth.

The bundle is produced by ``cd spa && npm run build`` into ``buildapp/static/``:
``index.html`` (served at /app/ with the user's initial substituted) plus hashed
assets under ``static/assets/`` (js, css, self-hosted fonts). Zero CDN.
"""

import asyncio
import os
from pathlib import Path

from litestar import Controller, Request, get
from litestar.enums import MediaType
from litestar.exceptions import NotFoundException
from litestar.response import Redirect, Response
from litestar.status_codes import HTTP_403_FORBIDDEN
from skrift.config import get_settings
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import releases
from buildapp.accounts import account_email
from buildapp.alpha_membership import is_alpha_member
from buildapp.desktop_auth import build_auth_guard
from buildapp.email_message import resolve_public_base_url
from buildapp.invite_pages import render_invite_only_page
from buildapp.session_auth import require_user, session_user_id

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


async def invite_only_response(user_id, db_session: AsyncSession) -> Response:
    """What a signed-in account with no redeemed invite sees at /app/. Names the address
    it is refusing, so the visitor can see they are signed in as the wrong one."""
    email = await account_email(db_session, user_id)
    return Response(
        render_invite_only_page(email),
        media_type=MediaType.HTML,
        status_code=HTTP_403_FORBIDDEN,
    )


class BuildController(Controller):
    path = "/app"

    @get("/")
    async def index(self, request: Request, db_session: AsyncSession) -> Response | Redirect:
        # Skrift auth via the shared session helper (a malformed session value
        # means "not logged in", never a 500). Unlike guarded API routes
        # (which answers 401), the SPA shell redirects to login — landing back on
        # the board afterwards, never the empty CMS root.
        user_id = session_user_id(request)
        if user_id is None:
            return Redirect("/auth/login?next=/app/")
        if not await is_alpha_member(db_session, user_id):
            return await invite_only_response(user_id, db_session)

        return await self._render_spa(user_id, db_session)

    @get("/desktop", guards=[build_auth_guard])
    async def desktop(self, request: Request, db_session: AsyncSession) -> Response:
        return await self._render_spa(require_user(request), db_session)

    @get("/downloads", guards=[build_auth_guard])
    async def downloads(self) -> dict:
        """Where an alpha member gets the bridge. Every URL and label comes from
        ``releases``; this handler only resolves the repo and this deployment's host."""
        return releases.downloads_payload(
            releases.releases_repo(os.environ),
            resolve_public_base_url(get_settings()),
        )

    @staticmethod
    async def _render_spa(user_id, db_session: AsyncSession) -> Response:
        email = await account_email(db_session, user_id) or "user"

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
