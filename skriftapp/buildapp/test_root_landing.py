"""The bare origin serves the marketing landing page — not a 404, not a bare
redirect. The page pitches Build and sends visitors into the SPA via /app/
(which owns its auth gate). Landing assets live in buildapp/landing/ — NOT in
buildapp/static/, which the SPA build wipes (emptyOutDir) — and are served from
/landing/* with the same traversal guard as the SPA assets."""

from __future__ import annotations

import asyncio
import re
from pathlib import Path

import pytest
from litestar.exceptions import NotFoundException
from litestar.response import Response

from buildapp.root_controller import LANDING_DIR, RootController

STYLESHEET_NAME = "landing.css"
FONT_NAME = "fonts/JetBrainsMono-latin.woff2"
BRAND_MARK_NAME = "brand-mark.svg"
SCREENSHOT_NAMES = (
    "assets/build-ide-screenshot-1240.png",
    "assets/build-ide-screenshot-1860.png",
)
SCREENSHOT_BYTE_BUDGET = 400_000
RETIRED_SCRIPT_NAME = "landing.js"

CI_WORKFLOW_PATH = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
SMOKE_CHECK_ASSET_PATTERN = r"getbuild\.ing/landing/([^\s\"']+)"
SMOKE_CHECK_PAGE_PHRASE = "agentic coding IDE"

APP_LINK = 'href="/app/"'
BRAND_LINK_MARKUP = (
    '<a class="brand" href="/app/">'
    '<img class="brand-mark" src="/landing/brand-mark.svg" alt="">build_</a>'
)
NAV_FEATURES_LINK = '<a href="#features">[features]</a>'
NAV_AGENTS_LINK = '<a href="#agents">[agents]</a>'
NAV_DOCS_LINK = '<a href="#">[docs]</a>'
NAV_CTA_MARKUP = (
    '<a class="button-primary button-primary--nav" href="#waitlist">REQUEST ACCESS</a>'
)
FOOTER_ASSURANCE_COPY = "LOCAL-FIRST // E2E ENCRYPTED"
FOOTER_COPYRIGHT_COPY = "© 2026 BUILD · GETBUILD.ING"

STYLESHEET_LINK = '<link rel="stylesheet" href="/landing/landing.css">'
FONT_PRELOAD_LINK = (
    '<link rel="preload" href="/landing/fonts/JetBrainsMono-latin.woff2" '
    'as="font" type="font/woff2" crossorigin>'
)
FONT_FACE_DECLARATION = (
    '@font-face{font-family:"JetBrains Mono";'
    'src:url("/landing/fonts/JetBrainsMono-latin.woff2") format("woff2");'
    "font-weight:400 800;font-style:normal;font-display:swap}"
)

SAME_ORIGIN_URL_PREFIX = '"/landing/'
STYLESHEET_URL_PATTERN = r"url\(([^)]*)\)"
ROOT_BLOCK_PATTERN = r":root\{[^}]*\}"
COLOUR_LITERAL_PATTERN = r"#[0-9a-fA-F]{3,8}\b|rgba?\(|gradient\("
BLOCK_COMMENT_OPENER = "/*"
LINE_COMMENT_OPENER = "//"
COMMENTABLE_SOURCE_SUFFIXES = (".css", ".js")

DESIGN_TOKENS = (
    "--color-accent",
    "--color-accent-hover",
    "--color-background",
    "--color-screenshot-frame",
    "--color-on-accent",
    "--color-alert",
    "--color-text-primary",
    "--color-text-secondary",
    "--color-text-muted",
    "--hairline-accent",
    "--hairline-neutral",
    "--surface-card",
    "--surface-card-hover",
    "--surface-input",
    "--surface-nav-solid",
    "--surface-menu",
    "--scanline-overlay",
    "--sweep-gradient",
    "--glow-text",
    "--glow-button",
    "--glow-button-hover",
    "--glow-nav-cta-hover",
    "--glow-card-active",
    "--glow-screenshot",
    "--easing-standard",
    "--duration-nav",
    "--duration-card-hover",
    "--duration-menu-toggle",
    "--duration-rail-transform",
    "--duration-rail-border",
    "--duration-dash",
    "--font-mono",
    "--content-max-width",
    "--content-padding-inline",
    "--viewport-height",
    "--nav-height",
    "--brand-mark-size",
    "--brand-gap",
    "--font-size-brand",
    "--letter-spacing-brand",
    "--dotfield-gap",
    "--hero-padding-block-start",
    "--hero-padding-block-end",
    "--screenshot-padding-block",
    "--features-padding-block",
    "--cta-padding-block",
    "--footer-padding",
    "--card-padding",
    "--agent-cell-padding",
    "--nav-link-padding",
    "--nav-cta-padding",
    "--nav-actions-gap",
    "--nav-links-gap",
    "--button-padding",
    "--input-padding",
    "--form-gap",
    "--bento-gap",
    "--rail-gap",
    "--rail-padding",
    "--rail-trailing-space",
    "--dash-gap",
    "--space-eyebrow-bottom",
    "--space-lead-top",
    "--space-form-top",
    "--space-note-top",
    "--space-section-header-bottom",
    "--space-section-label-top",
    "--space-cta-note-top",
    "--space-cta-form-top",
    "--space-card-header-bottom",
    "--space-card-title-bottom",
    "--space-agent-support-top",
    "--space-dashes-top",
    "--space-footer-gap",
    "--font-size-hero",
    "--line-height-hero",
    "--letter-spacing-hero",
    "--letter-spacing-heading",
    "--font-size-section-title",
    "--line-height-section-title",
    "--font-size-cta-title",
    "--line-height-cta-title",
    "--font-size-lead",
    "--line-height-lead",
    "--font-size-eyebrow",
    "--letter-spacing-eyebrow",
    "--line-height-hero-eyebrow",
    "--font-size-card-title",
    "--line-height-card-title",
    "--letter-spacing-card-title",
    "--font-size-card-body",
    "--line-height-card-body",
    "--font-size-nav-link",
    "--letter-spacing-nav-link",
    "--font-size-nav-cta",
    "--font-size-button",
    "--letter-spacing-button",
    "--font-size-input",
    "--font-size-note",
    "--letter-spacing-note",
    "--line-height-note",
    "--font-size-cta-note",
    "--letter-spacing-cta-note",
    "--font-size-success",
    "--letter-spacing-success",
    "--line-height-success",
    "--font-size-agent-name",
    "--letter-spacing-agent-name",
    "--font-size-agent-support",
    "--font-size-footer",
    "--letter-spacing-footer",
)


def _landing_text(name: str) -> str:
    return (LANDING_DIR / name).read_text()


def _landing_html() -> str:
    return asyncio.run(RootController.root.fn(None)).content


def test_root_serves_landing_html():
    response = asyncio.run(RootController.root.fn(None))
    assert isinstance(response, Response)
    assert response.media_type == "text/html"
    assert "Build" in response.content
    assert APP_LINK in response.content


def test_landing_page_references_only_same_origin_assets():
    """Strict CSP: no CDN. Every src/href in the page is a local path."""
    html = _landing_html()
    assert "https://" not in html
    assert "http://" not in html


def test_landing_asset_serves_stylesheet_with_media_type():
    response = RootController.landing_asset.fn(None, asset_path=STYLESHEET_NAME)
    assert isinstance(response, Response)
    assert response.media_type == "text/css"
    assert (LANDING_DIR / STYLESHEET_NAME).is_file()


def test_landing_asset_serves_font_with_media_type():
    response = RootController.landing_asset.fn(None, asset_path=FONT_NAME)
    assert isinstance(response, Response)
    assert response.media_type == "font/woff2"
    assert (LANDING_DIR / FONT_NAME).is_file()


def test_landing_asset_serves_brand_mark_with_media_type():
    response = RootController.landing_asset.fn(None, asset_path=BRAND_MARK_NAME)
    assert isinstance(response, Response)
    assert response.media_type == "image/svg+xml"
    assert (LANDING_DIR / BRAND_MARK_NAME).is_file()


def test_landing_asset_serves_screenshot_with_media_type():
    for screenshot_name in SCREENSHOT_NAMES:
        response = RootController.landing_asset.fn(None, asset_path=screenshot_name)
        assert isinstance(response, Response)
        assert response.media_type == "image/png"


def test_screenshot_renditions_stay_under_the_web_budget():
    for screenshot_name in SCREENSHOT_NAMES:
        assert (LANDING_DIR / screenshot_name).stat().st_size < SCREENSHOT_BYTE_BUDGET


def test_landing_asset_rejects_traversal():
    with pytest.raises(NotFoundException):
        RootController.landing_asset.fn(None, asset_path="../controllers.py")


def test_landing_asset_missing_file_404s():
    with pytest.raises(NotFoundException):
        RootController.landing_asset.fn(None, asset_path="nope.js")


def test_stylesheet_references_only_same_origin_urls():
    css = _landing_text(STYLESHEET_NAME)
    assert "https://" not in css
    assert "http://" not in css
    referenced = re.findall(STYLESHEET_URL_PATTERN, css)
    assert referenced
    for reference in referenced:
        assert reference.strip().startswith(SAME_ORIGIN_URL_PREFIX)


def test_stylesheet_declares_the_self_hosted_variable_font():
    assert FONT_FACE_DECLARATION in _landing_text(STYLESHEET_NAME)


def test_stylesheet_defines_every_design_token():
    css = _landing_text(STYLESHEET_NAME)
    root_block = re.search(ROOT_BLOCK_PATTERN, css)
    assert root_block is not None
    for token_name in DESIGN_TOKENS:
        assert f"{token_name}:" in root_block.group(0)


def test_colour_and_gradient_literals_live_only_in_the_token_blocks():
    remainder = re.sub(ROOT_BLOCK_PATTERN, "", _landing_text(STYLESHEET_NAME))
    assert re.search(COLOUR_LITERAL_PATTERN, remainder) is None


def test_page_has_no_inline_script_or_style_blocks():
    html = _landing_html()
    assert "<script" not in html
    assert "<style" not in html


def test_page_preloads_the_font_and_links_the_stylesheet():
    html = _landing_html()
    assert FONT_PRELOAD_LINK in html
    assert STYLESHEET_LINK in html


def test_brand_mark_links_into_the_app():
    assert BRAND_LINK_MARKUP in _landing_html()


def test_nav_links_anchor_to_features_and_agents_and_leave_docs_unlinked():
    html = _landing_html()
    assert NAV_FEATURES_LINK in html
    assert NAV_AGENTS_LINK in html
    assert NAV_DOCS_LINK in html


def test_nav_cta_links_to_the_waitlist_anchor():
    assert NAV_CTA_MARKUP in _landing_html()


def test_footer_copy_is_verbatim():
    html = _landing_html()
    assert FOOTER_ASSURANCE_COPY in html
    assert FOOTER_COPYRIGHT_COPY in html


def test_old_landing_script_is_gone():
    assert not (LANDING_DIR / RETIRED_SCRIPT_NAME).exists()


def test_landing_sources_carry_no_comments():
    for source in LANDING_DIR.rglob("*"):
        if source.suffix not in COMMENTABLE_SOURCE_SUFFIXES:
            continue
        text = source.read_text()
        assert BLOCK_COMMENT_OPENER not in text
        for line in text.splitlines():
            assert not line.strip().startswith(LINE_COMMENT_OPENER)


def test_deploy_smoke_check_targets_shipped_landing_assets():
    workflow = CI_WORKFLOW_PATH.read_text()
    smoke_checked_assets = re.findall(SMOKE_CHECK_ASSET_PATTERN, workflow)
    assert smoke_checked_assets
    for asset_path in smoke_checked_assets:
        assert (LANDING_DIR / asset_path).is_file()
    assert SMOKE_CHECK_PAGE_PHRASE in _landing_html()
