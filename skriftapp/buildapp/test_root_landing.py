"""The bare origin serves the marketing landing page — not a 404, not a bare
redirect. The page pitches Build and sends visitors into the SPA via /app/
(which owns its auth gate). Landing assets live in buildapp/landing/ — NOT in
buildapp/static/, which the SPA build wipes (emptyOutDir) — and are served from
/landing/* with the same traversal guard as the SPA assets."""

from __future__ import annotations

import asyncio

import pytest
from litestar.exceptions import NotFoundException
from litestar.response import Response

from buildapp.root_controller import LANDING_DIR, RootController


def test_root_serves_landing_html():
    response = asyncio.run(RootController.root.fn(None))
    assert isinstance(response, Response)
    assert response.media_type == "text/html"
    assert "Build" in response.content
    assert 'href="/app/"' in response.content


def test_landing_page_references_only_same_origin_assets():
    """Strict CSP: no CDN. Every src/href in the page is a local path."""
    html = asyncio.run(RootController.root.fn(None)).content
    assert "https://" not in html
    assert "http://" not in html


def test_landing_asset_serves_js_with_media_type():
    response = RootController.landing_asset.fn(None, asset_path="landing.js")
    assert isinstance(response, Response)
    assert response.media_type == "text/javascript"
    assert (LANDING_DIR / "landing.js").is_file()


def test_landing_asset_rejects_traversal():
    with pytest.raises(NotFoundException):
        RootController.landing_asset.fn(None, asset_path="../controllers.py")


def test_landing_asset_missing_file_404s():
    with pytest.raises(NotFoundException):
        RootController.landing_asset.fn(None, asset_path="nope.js")
