"""Root redirect: the bare origin sends visitors into the SPA.

Without this, GET / falls through to a 404 — a user typing the domain lands on
nothing. The SPA shell at /app/ owns the auth gate (redirecting anonymous
visitors to login), so the root only needs to point there.
"""

from litestar import Controller, get
from litestar.response import Redirect


class RootController(Controller):
    path = ""

    @get("/")
    async def root(self) -> Redirect:
        return Redirect("/app/")
