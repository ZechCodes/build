"""The bare origin must send visitors into the SPA, not 404. A GET / redirects
to /app/ (which then handles its own auth gate); no CMS/empty root is exposed."""

from __future__ import annotations

import asyncio

from litestar.response import Redirect

from buildapp.root_controller import RootController


def test_root_redirects_to_app():
    response = asyncio.run(RootController.root.fn(None))
    assert isinstance(response, Redirect)
    assert response.url == "/app/"
