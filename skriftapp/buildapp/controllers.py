"""Build app controller — serves the SPA bundle (built from spa/) behind Skrift auth.

The bundle is produced by ``cd spa && npm run build`` into ``buildapp/static/``:
``index.html`` (served at /app/ with the user's initial substituted) plus hashed
assets under ``static/assets/`` (js, css, self-hosted fonts). Zero CDN.
"""

from pathlib import Path
from uuid import UUID

from litestar import Controller, Request, get
from litestar.exceptions import NotFoundException
from litestar.response import Redirect, Response
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from skrift.db.models.user import User

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

    @get("/", sync_to_thread=False)
    async def index(self, request: Request, db_session: AsyncSession) -> Response | Redirect:
        # Skrift auth: a logged-in user has user_id in the encrypted session.
        user_id = request.session.get("user_id")
        if not user_id:
            # Land back on the board after login, never the empty CMS root.
            return Redirect("/auth/login?next=/app/")

        result = await db_session.execute(select(User).where(User.id == UUID(user_id)))
        user = result.scalar_one_or_none()
        email = getattr(user, "email", None) or "user"

        html = (STATIC_DIR / "index.html").read_text()
        html = html.replace("{{USER0}}", email[0:1].upper())
        return Response(html, media_type="text/html")

    @get("/static/{asset_path:path}", sync_to_thread=False)
    async def static_asset(self, asset_path: str) -> Response:
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
