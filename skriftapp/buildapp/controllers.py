"""Build app controller — serves the SPA bundle (built from spa/) behind Skrift auth.

The bundle is produced by ``cd spa && npm run build`` into ``buildapp/static/``:
``index.html`` (served at /app/ with the user's initial substituted) plus hashed
assets under ``static/assets/`` (js, css, self-hosted fonts). Zero CDN.
"""

import asyncio
from pathlib import Path

from litestar import Controller, Request, get, post
from litestar.di import Provide
from litestar.exceptions import NotFoundException
from litestar.response import Redirect, Response
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import download_tokens, releases
from buildapp.accounts import account_email
from buildapp.alpha_membership import is_alpha_member
from buildapp.clock import utc_now
from buildapp.desktop_auth import build_auth_guard, download_auth_guard
from buildapp.email_message import provide_public_base_url
from buildapp.invite_pages import APP_PATH, invite_only_outcome
from buildapp.release_assets import AssetSource, provide_asset_source
from buildapp.releases import DOWNLOAD_TOKEN_PARAM
from buildapp.session_auth import login_redirect, require_user, session_user_id

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
    dependencies = {
        "public_base_url": Provide(provide_public_base_url, sync_to_thread=False),
        "asset_source": Provide(provide_asset_source, sync_to_thread=False),
    }

    @get("/")
    async def index(self, request: Request, db_session: AsyncSession) -> Response | Redirect:
        # Skrift auth via the shared session helper (a malformed session value
        # means "not logged in", never a 500). Unlike guarded API routes
        # (which answers 401), the SPA shell redirects to login — landing back on
        # the board afterwards, never the empty CMS root.
        user_id = session_user_id(request)
        if user_id is None:
            return login_redirect(APP_PATH)
        if not await is_alpha_member(db_session, user_id):
            email = await account_email(db_session, user_id)
            return invite_only_outcome(email).response()

        return await self._render_spa(user_id, db_session)

    @get("/desktop", guards=[build_auth_guard])
    async def desktop(self, request: Request, db_session: AsyncSession) -> Response:
        return await self._render_spa(require_user(request), db_session)

    @get("/downloads", guards=[build_auth_guard])
    async def downloads(
        self,
        request: Request,
        db_session: AsyncSession,
        public_base_url: str,
        asset_source: AssetSource,
    ) -> dict:
        """Where an alpha member gets the bridge. Rendering the page mints the member
        a fresh install line, because the line is what they came for; every URL and
        label comes from ``releases``."""
        token = await download_tokens.mint(db_session, require_user(request), utc_now())
        return releases.downloads_payload(
            public_base_url, token, asset_source.releases_url
        )

    @post("/downloads/token", guards=[build_auth_guard])
    async def download_token(
        self, request: Request, db_session: AsyncSession, public_base_url: str
    ) -> Response:
        """A fresh install line for a page that has been open a while. Same shape as
        the gateway token: empty body, no CSRF header, 201 with the secret once."""
        token = await download_tokens.mint(db_session, require_user(request), utc_now())
        return Response(
            {
                "token": token,
                "install_command": releases.install_command(public_base_url, token),
                "expires_in_s": download_tokens.EXPIRES_IN_S,
            },
            status_code=201,
        )

    @get("/downloads/{asset:str}", guards=[download_auth_guard])
    async def download_asset(
        self,
        asset: str,
        request: Request,
        db_session: AsyncSession,
        asset_source: AssetSource,
    ) -> Response:
        """One release asset, to a member's browser or to their install script. The
        segment must be one this app publishes, the source decides how the bytes get
        there, and a tarball — the last thing an install fetches — spends the token
        that opened it."""
        downloadable = releases.DOWNLOADABLE.get(asset)
        if downloadable is None:
            raise NotFoundException("no such download")
        response = await asset_source.deliver(downloadable.name)
        if downloadable.spends_token:
            await download_tokens.spend(
                db_session,
                request.query_params.get(DOWNLOAD_TOKEN_PARAM),
                require_user(request),
            )
        return response

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
