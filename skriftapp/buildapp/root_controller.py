"""Public landing, documentation, and same-origin landing assets."""

import asyncio

from litestar import Controller, Request, get
from litestar.di import Provide
from litestar.enums import MediaType
from litestar.exceptions import ClientException, NotFoundException
from litestar.response import Response

from buildapp import download_tokens, releases
from buildapp.email_message import provide_public_base_url
from buildapp.landing_page import (
    LANDING_DIR,
    fill_slots,
    read_landing_file,
    render_shell,
)
from buildapp.landing_content import (
    DOCS_PATH,
    PRIVACY_PATH,
    render_docs_body,
    render_practical_content,
    render_privacy_body,
)

LANDING_PAGE_NAME = "index.html"
PRACTICAL_SLOT_NAME = "practical_content"
LANDING_TITLE = "Build — Any screen. Your call."
LANDING_DESCRIPTION = "Your coding agents. Your hardware. Any screen. Free and open source."
LANDING_SCRIPTS = '<script type="module" src="/landing/main.js"></script>'
LANDING_HEAD = '<link rel="stylesheet" href="/landing/cinematic.css">'
SOCIAL_HEAD = (
    '<meta property="og:title" content="Build — Set the work in motion.">'
    '<meta property="og:description" content="Your coding agents. Your hardware. Any screen.">'
    '<meta property="og:type" content="website">'
    '<meta property="og:image" content="https://getbuild.ing/landing/assets/social-preview.png">'
    '<meta property="og:image:width" content="1200">'
    '<meta property="og:image:height" content="630">'
    '<meta name="twitter:card" content="summary_large_image">'
)
#: What a query string that is not even shaped like a token is answered with.
MALFORMED_TOKEN_DETAIL = "malformed download token"

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
    practical = render_practical_content().rstrip("\n")
    return render_shell(
        title=LANDING_TITLE,
        description=LANDING_DESCRIPTION,
        body=fill_slots(
            read_landing_file(LANDING_PAGE_NAME),
            {PRACTICAL_SLOT_NAME: practical},
        ),
        scripts=LANDING_SCRIPTS,
        head=LANDING_HEAD + SOCIAL_HEAD,
        footer="",
        body_class="cinematic-page",
    )


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


class RootController(Controller):
    path = ""
    dependencies = {
        "public_base_url": Provide(provide_public_base_url, sync_to_thread=False),
    }

    @get("/")
    async def root(self) -> Response:
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
    async def install_script(self, request: Request, public_base_url: str) -> Response:
        """The one-liner's target: ``curl -fsSL "<host>/install.sh?t=<token>" | sh``.

        Serves the script this image ships, filled in for this deployment: its own
        origin and the caller's download token. Never a redirect to GitHub — the
        repository may be private — and never a database read: the download routes are
        the one judge of a token. A token that is not even shaped like one is refused
        outright, because the value is substituted into a shell script."""
        token = request.query_params.get(releases.DOWNLOAD_TOKEN_PARAM, "")
        if token and not download_tokens.is_well_formed(token):
            raise ClientException(MALFORMED_TOKEN_DETAIL)
        script = await asyncio.to_thread(releases.INSTALL_SCRIPT_FILE.read_text)
        return Response(
            releases.render_install_script(script, public_base_url, token),
            media_type=MediaType.TEXT,
            headers={"Cache-Control": "no-store"},
        )

    @get("/landing/{asset_path:path}", sync_to_thread=True)
    def landing_asset(self, asset_path: str) -> Response:
        resolved = (LANDING_DIR / asset_path.lstrip("/")).resolve()
        if not resolved.is_relative_to(LANDING_DIR.resolve()) or not resolved.is_file():
            raise NotFoundException()
        media_type = LANDING_MEDIA_TYPES.get(resolved.suffix, "application/octet-stream")
        return Response(resolved.read_bytes(), media_type=media_type)
