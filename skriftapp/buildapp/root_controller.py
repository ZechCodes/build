"""Root: GET / renders the landing body through the shared page shell, and /landing/*
serves the landing assets behind a traversal guard. The waitlist block appears twice on
the page, so it is written once in ``waitlist.html`` and substituted into both slots."""

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

LANDING_PAGE_NAME = "index.html"
WAITLIST_FRAGMENT_NAME = "waitlist.html"
WAITLIST_SLOT_NAME = "waitlist"
LANDING_TITLE = "Build — Ship more. Ship confidence."
LANDING_DESCRIPTION = (
    "Build turns an issue into shipped code. An agent plans and builds it on your machi"
    "ne, and a review agent lays the changes out so you know exactly what went out and why."
)
LANDING_SCRIPTS = '<script type="module" src="/landing/main.js"></script>'
#: What a query string that is not even shaped like a token is answered with.
MALFORMED_TOKEN_DETAIL = "malformed download token"

LANDING_MEDIA_TYPES = {
    ".js": "text/javascript",
    ".css": "text/css",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".ico": "image/x-icon",
    ".woff2": "font/woff2",
}


def render_landing_page() -> str:
    waitlist = read_landing_file(WAITLIST_FRAGMENT_NAME).rstrip("\n")
    return render_shell(
        title=LANDING_TITLE,
        description=LANDING_DESCRIPTION,
        body=fill_slots(
            read_landing_file(LANDING_PAGE_NAME), {WAITLIST_SLOT_NAME: waitlist}
        ),
        scripts=LANDING_SCRIPTS,
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
