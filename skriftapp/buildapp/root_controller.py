"""Public landing, documentation, and same-origin landing assets.

The homepage is not rendered here. `landing/` at the repo root is an Astro project
whose `npm run build` emits a complete document into ``landing/generated/``; this
module serves that document and fills the only two values that cannot be static.
`/docs` and `/privacy` are still Python-rendered bodies in the shared shell.
"""

import asyncio
from pathlib import Path

from litestar import Controller, get
from litestar.di import Provide
from litestar.enums import MediaType
from litestar.exceptions import NotFoundException
from litestar.response import Response
from litestar.status_codes import HTTP_503_SERVICE_UNAVAILABLE

from buildapp import releases
from buildapp.email_message import provide_public_base_url
from buildapp.email_template import EMAIL_ASSET_DIRECTORY
from buildapp.landing_page import (
    LANDING_DIR,
    fill_slots,
    render_shell,
)
from buildapp.landing_content import (
    DOCS_PATH,
    PRIVACY_PATH,
    render_docs_body,
    render_homepage_slots,
    render_privacy_body,
)

#: The Astro build output. Gitignored, like the SPA's bundle: built by the node stage
#: in skriftapp/Containerfile, or by `cd landing && npm run build` in a checkout.
GENERATED_PAGE = LANDING_DIR / "generated" / "index.html"
#: What `/` says when the build never ran. One sentence, no verb prefix — it is read
#: by a person in a browser, not by a log.
UNBUILT_LANDING_MESSAGE = "The landing page has not been built yet."
#: Only /docs and /privacy still render through the shell, and cinematic.css is their
#: stylesheet. The homepage carries its own.
LANDING_HEAD = '<link rel="stylesheet" href="/landing/cinematic.css">'

#: Email images sit at stable URLs in mail already sent, so a mail client may keep them a
#: year. Not ``immutable``: a changed render keeps its name, and a year is the bound on
#: how long an old copy can be shown. See design/email/README.md.
EMAIL_ASSET_CACHE_CONTROL = "public, max-age=31536000"

LANDING_MEDIA_TYPES = {
    ".js": "text/javascript",
    ".css": "text/css",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".webp": "image/webp",
    ".glb": "model/gltf-binary",
    ".json": "application/json",
    ".ico": "image/x-icon",
    ".woff2": "font/woff2",
}


def render_landing_page() -> str:
    """The generated document, byte for byte, with its two server slots filled."""
    return fill_slots(GENERATED_PAGE.read_text(), render_homepage_slots())


def render_docs_page() -> str:
    return render_shell(
        title="Docs — Build",
        description="Set up a Build host and check the current coding harness integrations.",
        body=render_docs_body(),
        scripts="",
        head=LANDING_HEAD,
        body_class="cinematic-page public-doc-page",
    )


def render_privacy_page() -> str:
    return render_shell(
        title="Architecture and privacy — Build",
        description="What Build stores, where agents run, and how browsers connect to hosts.",
        body=render_privacy_body(),
        scripts="",
        head=LANDING_HEAD,
        body_class="cinematic-page public-doc-page",
    )


def landing_cache_headers(asset: Path) -> dict[str, str]:
    """Only the email images are cached long; the page's own assets are revalidated,
    because their names do not change when a deploy changes them."""
    if asset.parent == (LANDING_DIR / EMAIL_ASSET_DIRECTORY).resolve():
        return {"Cache-Control": EMAIL_ASSET_CACHE_CONTROL}
    return {}


class RootController(Controller):
    path = ""
    dependencies = {
        "public_base_url": Provide(provide_public_base_url, sync_to_thread=False),
    }

    @get("/")
    async def root(self) -> Response:
        if not GENERATED_PAGE.is_file():
            return Response(
                UNBUILT_LANDING_MESSAGE,
                media_type=MediaType.TEXT,
                status_code=HTTP_503_SERVICE_UNAVAILABLE,
            )
        html = await asyncio.to_thread(render_landing_page)
        return Response(html, media_type=MediaType.HTML)

    @get(DOCS_PATH)
    async def docs(self) -> Response:
        html = await asyncio.to_thread(render_docs_page)
        return Response(html, media_type=MediaType.HTML)

    @get(PRIVACY_PATH)
    async def privacy(self) -> Response:
        html = await asyncio.to_thread(render_privacy_page)
        return Response(html, media_type=MediaType.HTML)

    @get(releases.INSTALL_SCRIPT_PATH)
    async def install_script(self) -> Response:
        return await self._installer(releases.INSTALL_SCRIPT_FILE)

    @get(releases.DESKTOP_INSTALL_SCRIPT_PATH)
    async def desktop_install_script(self) -> Response:
        return await self._installer(releases.DESKTOP_INSTALL_SCRIPT_FILE)

    @staticmethod
    async def _installer(path) -> Response:
        script = await asyncio.to_thread(path.read_text)
        return Response(
            releases.render_install_script(script),
            media_type=MediaType.TEXT,
            headers={"Cache-Control": "no-cache"},
        )

    @get("/landing/{asset_path:path}", sync_to_thread=True)
    def landing_asset(self, asset_path: str) -> Response:
        resolved = (LANDING_DIR / asset_path.lstrip("/")).resolve()
        if not resolved.is_relative_to(LANDING_DIR.resolve()) or not resolved.is_file():
            raise NotFoundException()
        media_type = LANDING_MEDIA_TYPES.get(resolved.suffix, "application/octet-stream")
        return Response(
            resolved.read_bytes(), media_type=media_type, headers=landing_cache_headers(resolved)
        )
