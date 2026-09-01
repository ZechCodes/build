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


DOT_FIELD_MARKUP = '<div class="dot-field"><canvas data-dot-field></canvas></div>'
HERO_EYEBROW_MARKUP = (
    '<p class="eyebrow hero-eyebrow">PRIVATE BETA — INVITES<br>GOING OUT WEEKLY</p>'
)
HERO_EYEBROW_BREAK_HIDDEN_RULE = ".hero-eyebrow br{display:none}"
HERO_EYEBROW_BREAK_SHOWN_RULE = ".hero-eyebrow br{display:inline}"
HERO_TITLE_MARKUP = (
    '<h1 class="hero-title">Ship more.<br>'
    '<span class="title-accent">Babysit less.</span></h1>'
)
HERO_LEAD_MARKUP = (
    '<p class="lead">Build is the agentic coding IDE for teams. It surfaces the work '
    "that needs you — reviews, decisions, direction — and dispatches everything else "
    "to your agents. Not public yet.</p>"
)
HERO_NOTE_MARKUP = (
    '<p class="waitlist-note">One email when your invite is ready. Nothing else.</p>'
)

SCREENSHOT_MARKUP = (
    '<img src="/landing/assets/build-ide-screenshot-1240.png" '
    'srcset="/landing/assets/build-ide-screenshot-1240.png 1240w, '
    '/landing/assets/build-ide-screenshot-1860.png 1860w" '
    'sizes="(max-width: 640px) 100vw, (max-width: 1280px) calc(100vw - 80px), 1160px" '
    'width="1860" height="1629" loading="lazy" decoding="async" '
    'alt="The Build IDE — commit history, diff review, and a live agent conversation">'
)

AGENTS_LABEL_MARKUP = '<p class="eyebrow agents-label">RUNS YOUR AGENTS</p>'
AGENT_NAMES = ("Claude Code", "Codex", "Pi", "OpenCode")
AGENT_SUPPORT_LABEL = '<span class="agent-support">supported</span>'

FEATURES_TITLE_MARKUP = (
    '<h2 class="section-title">Everything agents need.<br>'
    "Nothing <span class=\"title-accent\">you don't.</span></h2>"
)
FEATURES_LABEL_MARKUP = '<span class="eyebrow section-label">FEATURES /05</span>'
FEATURE_CARDS = (
    (
        "/01",
        "ISSUES",
        "An inbox, not a backlog",
        "Issues live next to the code. Agents pick them up on their own — your inbox "
        "shows only the work waiting on a human. When it's empty, you're done.",
    ),
    (
        "/02",
        "VERSION CONTROL",
        "Worktrees that manage themselves",
        "Every agent works in its own worktree. Fetch, commit, review, merge — "
        "without leaving the conversation.",
    ),
    (
        "/03",
        "REVIEW",
        "Diff-first review",
        "Inline comments on any line, threaded with your team and your agents. "
        "Approve, redirect, or take over.",
    ),
    (
        "/04",
        "WORKFLOWS · RLM",
        "Workflows that reshape around the work",
        "The RLM routes tasks, retries failures, and re-plans as the work changes — "
        "escalating only when it genuinely needs a human. Your team sees the same "
        "board, live.",
    ),
    (
        "/05",
        "AGENTS",
        "Any agent, side by side",
        "Claude Code, Codex, Pi, and OpenCode run in parallel sessions — one surface, "
        "one shared history, one place to look.",
    ),
)
CARD_SPAN_MODIFIERS = ("", "", "--span-2", "--span-4", "--span-6")
ARTICLE_TAG_PATTERN = r"<article[^>]*>"
SPAN_MODIFIER_PATTERN = r"--span-\d"

ACTIVE_RAIL_DASH_MARKUP = '<i class="rail-dash is-active" data-rail-dash></i>'
IDLE_RAIL_DASH_MARKUP = '<i class="rail-dash" data-rail-dash></i>'
IDLE_RAIL_DASH_COUNT = 4

CTA_TITLE_MARKUP = (
    '<h2 class="cta-title">Get in <span class="title-accent">early.</span></h2>'
)
CTA_NOTE_MARKUP = (
    '<p class="cta-note">PRIVATE BETA · LOCAL-FIRST · E2E ENCRYPTED</p>'
)

WAITLIST_ERROR_MARKUP = (
    '<p class="waitlist-error" role="alert" hidden>✗ COULDN’T ADD YOU — TRY AGAIN.</p>'
)
WAITLIST_SUCCESS_MARKUP = (
    '<p class="waitlist-success" role="status" hidden>✓ YOU’RE ON THE LIST — '
    "<span data-waitlist-email></span></p>"
)
WAITLIST_COMPONENT_PATTERN = r'<div class="waitlist" data-waitlist>.*?</div>'
WAITLIST_INSTANCE_COUNT = 2

NAV_ELEMENT_PATTERN = r"<nav\b.*?</nav>"
ANCHOR_HREF_PATTERN = r'href="#([^"]*)"'

MEDIA_BLOCK_OPENER = "@media"
RULE_PATTERN = r"([^{}]+)\{[^{}]*\}"
COMPONENT_LAYER_SELECTORS = (
    ".eyebrow",
    ".section-header",
    ".section-title",
    ".title-accent",
    ".rule-top",
    ".rule-bottom",
    ".button-primary",
    ".button-primary--nav",
    ".waitlist",
    ".waitlist-form",
    ".waitlist-input",
    ".waitlist-note",
    ".waitlist-success",
    ".waitlist-error",
    ".feature-cards",
    ".feature-card",
    ".feature-card--span-2",
    ".feature-card--span-4",
    ".feature-card--span-6",
    ".card-header",
    ".card-category",
    ".card-title",
    ".card-body",
    ".rail-dashes",
    ".agents-grid",
    ".agents-label",
    ".agent-cell",
    ".agent-support",
    ".nav-bar",
    ".nav-links",
    ".nav-actions",
    ".nav-menu-toggle",
    ".nav-menu-toggle-bar",
)


def _rules_outside_media_blocks(css: str) -> str:
    kept: list[str] = []
    index = 0
    while index < len(css):
        media_start = css.find(MEDIA_BLOCK_OPENER, index)
        if media_start == -1:
            kept.append(css[index:])
            break
        kept.append(css[index:media_start])
        depth = 0
        cursor = css.index("{", media_start)
        while cursor < len(css):
            if css[cursor] == "{":
                depth += 1
            elif css[cursor] == "}":
                depth -= 1
                if depth == 0:
                    break
            cursor += 1
        index = cursor + 1
    return "".join(kept)


def _declared_selectors(css: str) -> list[str]:
    return [
        selector.strip()
        for selector_list in re.findall(RULE_PATTERN, css)
        for selector in selector_list.split(",")
    ]


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


def test_hero_copy_is_verbatim():
    html = _landing_html()
    assert HERO_EYEBROW_MARKUP in html
    assert HERO_TITLE_MARKUP in html
    assert HERO_LEAD_MARKUP in html
    assert HERO_NOTE_MARKUP in html


def test_hero_eyebrow_carries_the_mobile_line_break():
    css = _landing_text(STYLESHEET_NAME)
    assert HERO_EYEBROW_BREAK_HIDDEN_RULE in css
    assert HERO_EYEBROW_BREAK_SHOWN_RULE in css
    assert HERO_EYEBROW_BREAK_SHOWN_RULE not in _rules_outside_media_blocks(css)


def test_hero_hosts_the_dot_field_canvas():
    assert DOT_FIELD_MARKUP in _landing_html()


def test_screenshot_uses_both_renditions_with_the_designed_alt_text():
    assert SCREENSHOT_MARKUP in _landing_html()


def test_every_agent_name_is_listed_with_supported():
    html = _landing_html()
    assert AGENTS_LABEL_MARKUP in html
    for agent_name in AGENT_NAMES:
        assert f'<div class="agent-cell">{agent_name}{AGENT_SUPPORT_LABEL}</div>' in html


def test_features_header_copy_is_verbatim():
    html = _landing_html()
    assert FEATURES_TITLE_MARKUP in html
    assert FEATURES_LABEL_MARKUP in html


def test_every_feature_card_number_category_title_and_body_is_verbatim():
    html = _landing_html()
    for number, category, title, body in FEATURE_CARDS:
        assert (
            f'<div class="card-header eyebrow">'
            f'<span class="card-number">{number}</span>'
            f'<span class="card-category">{category}</span></div>' in html
        )
        assert f'<h3 class="card-title">{title}</h3>' in html
        assert f'<p class="card-body">{body}</p>' in html


def test_feature_cards_carry_the_designed_span_modifiers():
    article_tags = re.findall(ARTICLE_TAG_PATTERN, _landing_html())
    assert len(article_tags) == len(CARD_SPAN_MODIFIERS)
    for article_tag, expected_modifier in zip(article_tags, CARD_SPAN_MODIFIERS):
        found_modifiers = re.findall(SPAN_MODIFIER_PATTERN, article_tag)
        assert found_modifiers == ([expected_modifier] if expected_modifier else [])


def test_rail_has_five_dashes_with_the_first_active():
    html = _landing_html()
    assert html.count(ACTIVE_RAIL_DASH_MARKUP) == 1
    assert html.count(IDLE_RAIL_DASH_MARKUP) == IDLE_RAIL_DASH_COUNT
    assert html.index(ACTIVE_RAIL_DASH_MARKUP) < html.index(IDLE_RAIL_DASH_MARKUP)


def test_cta_section_copy_is_verbatim():
    html = _landing_html()
    assert CTA_TITLE_MARKUP in html
    assert CTA_NOTE_MARKUP in html


def test_waitlist_error_and_success_copy_are_verbatim_and_hidden():
    html = _landing_html()
    assert html.count(WAITLIST_ERROR_MARKUP) == WAITLIST_INSTANCE_COUNT
    assert html.count(WAITLIST_SUCCESS_MARKUP) == WAITLIST_INSTANCE_COUNT


def test_both_waitlist_forms_use_identical_component_markup():
    components = re.findall(WAITLIST_COMPONENT_PATTERN, _landing_html(), re.S)
    assert len(components) == WAITLIST_INSTANCE_COUNT
    assert components[0] == components[1]


def test_anchored_sections_exist_for_every_nav_link():
    html = _landing_html()
    nav_element = re.search(NAV_ELEMENT_PATTERN, html, re.S)
    assert nav_element is not None
    anchored_names = [
        name for name in re.findall(ANCHOR_HREF_PATTERN, nav_element.group(0)) if name
    ]
    assert anchored_names
    for anchored_name in anchored_names:
        assert f'id="{anchored_name}"' in html


def test_section_rules_declare_no_selector_owned_by_the_component_layer():
    declared = _declared_selectors(
        _rules_outside_media_blocks(_landing_text(STYLESHEET_NAME))
    )
    for owned_selector in COMPONENT_LAYER_SELECTORS:
        assert declared.count(owned_selector) == 1


def test_deploy_smoke_check_targets_shipped_landing_assets():
    workflow = CI_WORKFLOW_PATH.read_text()
    smoke_checked_assets = re.findall(SMOKE_CHECK_ASSET_PATTERN, workflow)
    assert smoke_checked_assets
    for asset_path in smoke_checked_assets:
        assert (LANDING_DIR / asset_path).is_file()
    assert SMOKE_CHECK_PAGE_PHRASE in _landing_html()
