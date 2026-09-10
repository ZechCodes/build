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

from buildapp.landing_page import SHELL_NAME, slot_placeholder
from buildapp.root_controller import (
    LANDING_DIR,
    LANDING_PAGE_NAME,
    WAITLIST_FRAGMENT_NAME,
    WAITLIST_SLOT_NAME,
    RootController,
)
from buildapp.unsubscribe_pages import (
    render_confirm_page,
    render_invalid_page,
    render_removed_page,
)

STYLESHEET_NAME = "landing.css"
ENTRY_MODULE_NAME = "main.js"
FONT_NAME = "fonts/JetBrainsMono-latin.woff2"
BRAND_MARK_NAME = "brand-mark.svg"
SCREENSHOT_NAMES = (
    "assets/build-ide-screenshot-1240.png",
    "assets/build-ide-screenshot-1744.png",
    "assets/build-ide-screenshot-mobile-640.png",
    "assets/build-ide-screenshot-mobile-921.png",
)
SCREENSHOT_BYTE_BUDGET = 400_000
ASSET_MEDIA_TYPES = (
    (STYLESHEET_NAME, "text/css"),
    (ENTRY_MODULE_NAME, "text/javascript"),
    (FONT_NAME, "font/woff2"),
    (BRAND_MARK_NAME, "image/svg+xml"),
    *((screenshot_name, "image/png") for screenshot_name in SCREENSHOT_NAMES),
)
RETIRED_SCRIPT_NAME = "landing.js"

CI_WORKFLOW_PATH = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
SMOKE_CHECK_ASSET_PATTERN = r"getbuild\.ing/landing/([^\s\"']+)"
SMOKE_CHECK_PAGE_PHRASE = "Ship confidence"
LANDING_TITLE_MARKUP = "<title>Build — Ship more. Ship confidence.</title>"

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

ENTRY_SCRIPT_MARKUP = '<script type="module" src="/landing/main.js"></script>'
SCRIPT_ELEMENT_PATTERN = r"<script\b([^>]*)>(.*?)</script>"
MODULE_IMPORT_PATTERN = r'from\s+"\./([^"]+)"'
DOCUMENT_GLOBAL = "document"
MODULE_SUFFIX = "*.js"

SAME_ORIGIN_URL_PREFIX = '"/landing/'
STYLESHEET_URL_PATTERN = r"url\(([^)]*)\)"
ROOT_BLOCK_PATTERN = r":root\{[^}]*\}"
COLOUR_LITERAL_PATTERN = r"#[0-9a-fA-F]{3,8}\b|rgba?\(|gradient\("
MEDIA_PRELUDE_PATTERN = r"@media[^{]*\{"
SIZE_LITERAL_PATTERN = r"\d(?:\.\d+)?(?:px|rem|em|vw|vh)\b"
BLOCK_COMMENT_OPENER = "/*"
MARKUP_COMMENT_OPENER = "<!--"
LINE_COMMENT_OPENER = "//"
COMMENT_OPENERS_BY_SUFFIX = {
    ".css": BLOCK_COMMENT_OPENER,
    ".js": BLOCK_COMMENT_OPENER,
    ".html": MARKUP_COMMENT_OPENER,
}
DOCTYPE_DECLARATION = "<!doctype"
HEAD_ELEMENT_OPENER = "<head"
MARKUP_SUFFIX = "*.html"
CONFIRM_PAGE_ADDRESS = "a@b.co"
CONFIRM_PAGE_TOKEN = "token"

DESIGN_TOKENS = (
    "--color-accent-channels",
    "--color-accent",
    "--color-accent-hover",
    "--color-background",
    "--color-screenshot-frame",
    "--fade-screenshot",
    "--color-on-accent",
    "--color-alert",
    "--color-text-primary",
    "--color-text-secondary",
    "--color-text-muted",
    "--hairline-accent",
    "--hairline-neutral",
    "--hairline",
    "--hairline-transparent",
    "--surface-card",
    "--surface-card-hover",
    "--surface-input",
    "--surface-nav-solid",
    "--surface-menu",
    "--surface-blur",
    "--scanline-overlay",
    "--scanline-opacity",
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
    "--space-screenshot-top",
    "--flow-columns",
    "--flow-col-gap",
    "--flow-row-gap",
    "--flow-line-width",
    "--flow-link",
    "--flow-node-padding",
    "--font-size-flow-title",
    "--line-height-flow-title",
    "--font-size-flow-body",
    "--line-height-flow-body",
    "--space-flow-header-bottom",
    "--space-flow-title-bottom",
    "--screenshot-crop-aspect",
    "--screenshot-side-height",
    "--hero-side-copy-width",
    "--features-padding-block",
    "--cta-padding-block",
    "--footer-padding",
    "--card-padding",
    "--agent-cell-padding",
    "--agents-label-column",
    "--nav-link-padding",
    "--nav-cta-padding",
    "--nav-actions-gap",
    "--nav-links-gap",
    "--menu-toggle-size",
    "--menu-toggle-bar-width",
    "--menu-toggle-bar-height",
    "--menu-toggle-bar-gap",
    "--menu-toggle-bar-offset",
    "--button-padding",
    "--input-padding",
    "--form-gap",
    "--lead-max-width",
    "--waitlist-max-width",
    "--waitlist-input-min-width",
    "--bento-gap",
    "--rail-gap",
    "--rail-padding",
    "--rail-trailing-space",
    "--rail-perspective",
    "--rail-card-width",
    "--rail-card-max-width",
    "--rail-dash-width",
    "--rail-dash-height",
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
    "--font-size-card-title",
    "--line-height-card-title",
    "--letter-spacing-card-title",
    "--font-size-card-body",
    "--line-height-card-body",
    "--font-size-nav-link",
    "--letter-spacing-nav-link",
    "--font-size-nav-cta",
    "--letter-spacing-nav-cta",
    "--font-size-button",
    "--letter-spacing-button",
    "--font-size-input",
    "--font-size-note",
    "--letter-spacing-note",
    "--line-height-note",
    "--font-size-cta-note",
    "--line-height-cta-note",
    "--letter-spacing-cta-note",
    "--font-size-success",
    "--letter-spacing-success",
    "--line-height-success",
    "--font-size-agent-name",
    "--letter-spacing-agent-name",
    "--font-size-footer",
    "--letter-spacing-footer",
    "--opacity-disabled",
)


DOT_FIELD_MARKUP = '<div class="dot-field"><canvas data-dot-field></canvas></div>'
HERO_EYEBROW_MARKUP = '<p class="eyebrow hero-eyebrow">PRIVATE BETA</p>'
HERO_SECTION_OPENER = '<section class="hero rule-bottom" data-hero>'
HERO_RULE = (
    ".hero{position:relative;overflow:hidden;display:flex;flex-direction:column;"
    "text-align:center;padding-top:var(--hero-padding-block-start)}"
)
HERO_VIEWPORT_LOCK_RULE = ".hero{min-height:var(--viewport-height)}"
HERO_STAGE_RULE = (
    ".hero-stage{flex:1;display:flex;flex-direction:column;align-items:center}"
)
HERO_COPY_CENTERED_RULE = ".hero-content{margin-block:auto}"
HERO_LAYERS_RULE = ".hero-stage,.agents-rail{position:relative;z-index:1}"
AGENTS_RAIL_MARKUP = '<section class="agents-rail rule-top" id="agents">'
AGENTS_RAIL_RULE = ".agents-rail{background:var(--color-background)}"
SCREENSHOT_FADE_RULE = (
    '.screenshot-frame::after{content:"";position:absolute;inset:0;'
    "background:var(--fade-screenshot);pointer-events:none}"
)
HERO_SIDE_MEDIA_PRELUDE = "@media (min-width:1024px) and (max-height:1199px){"
HERO_SIDE_RULES = (
    ".hero-stage{flex-direction:row;align-items:flex-end;text-align:left;max-width:none}",
    ".hero-content,.hero-screenshot{flex:1 1 0;min-width:0}",
    ".hero-content{display:grid;justify-content:center;"
    "grid-template-columns:minmax(0,var(--hero-side-copy-width))}",
    ".hero-content .lead,.hero-content .waitlist{margin-inline:0}",
    ".hero-screenshot{align-self:flex-end;margin-top:0}",
    ".screenshot-frame{width:max-content;aspect-ratio:auto;height:var(--screenshot-side-height)}",
    ".screenshot-frame img{width:auto;height:100%}",
)
SCREENSHOT_FADE_TOKEN = "--fade-screenshot:linear-gradient(to bottom,transparent 50%,#030604)"
HERO_TITLE_MARKUP = (
    '<h1 class="section-title section-title--hero">Ship more.<br>'
    '<span class="title-accent title-accent--glow">Ship confidence.</span></h1>'
)
HERO_LEAD_COPY = (
    "Build turns an issue into shipped code. An agent plans and builds it on your machi"
    "ne, and a review agent lays the changes out so you know exactly what went out and why."
)
LANDING_DESCRIPTION_MARKUP = '<meta name="description" content="' + HERO_LEAD_COPY + '">'
HERO_LEAD_MARKUP = (
    '<p class="lead">' + HERO_LEAD_COPY + "</p>"
)
HERO_NOTE_MARKUP = (
    '<p class="note">One email to confirm, one when your invite is ready. '
    "Nothing else.</p>"
)

SCREENSHOT_MARKUP = (
    "<picture>\n"
    '    <source media="(max-width: 640px)" '
    'srcset="/landing/assets/build-ide-screenshot-mobile-640.png 640w, '
    '/landing/assets/build-ide-screenshot-mobile-921.png 921w" '
    'sizes="100vw" width="921" height="1880">\n'
    '    <img src="/landing/assets/build-ide-screenshot-1240.png" '
    'srcset="/landing/assets/build-ide-screenshot-1240.png 1240w, '
    '/landing/assets/build-ide-screenshot-1744.png 1744w" '
    'sizes="(max-width: 1280px) calc(100vw - 80px), 1160px" '
    'width="1744" height="965" decoding="async" '
    'alt="The Build IDE — commit history, diff review, and a live agent conversation">\n'
    "  </picture>"
)
HERO_SCREENSHOT_OPENER = '<div class="hero-screenshot">'
HERO_SCREENSHOT_RULE = ".hero-screenshot{align-self:stretch;margin-top:var(--space-screenshot-top)}"
HERO_SCREENSHOT_MOBILE_RULE = ".hero-screenshot{margin-inline:calc(var(--content-padding-inline) * -1)}"
SCREENSHOT_FRAME_RULE = (
    ".screenshot-frame{position:relative;background:var(--color-screenshot-frame);"
    "aspect-ratio:var(--screenshot-crop-aspect);overflow:hidden}"
)
SCREENSHOT_GAP_DESKTOP_TOKEN = "--space-screenshot-top:64px"
SCREENSHOT_GAP_MOBILE_TOKEN = "--space-screenshot-top:28px"
SCREENSHOT_CROP_DESKTOP_TOKEN = "--screenshot-crop-aspect:1744 / 800"
SCREENSHOT_CROP_MOBILE_TOKEN = "--screenshot-crop-aspect:921 / 900"

AGENTS_LABEL_MARKUP = (
    '<p class="eyebrow eyebrow--muted agents-label">USE YOUR HARNESS</p>'
)
AGENTS_LABEL_RULE = (
    ".agents-label{display:flex;align-items:center;padding:var(--agent-cell-padding)}"
)
AGENTS_LABEL_MOBILE_RULE = ".agents-label{grid-column:1/-1;padding-bottom:0}"
AGENTS_GRID_INTERMEDIATE_RULE = (
    ".agents-grid{grid-template-columns:auto repeat(4,minmax(0,1fr))}"
)
HERO_FONT_SIZE_DESKTOP_TOKEN = "--font-size-hero:76px"
HERO_FONT_SIZE_INTERMEDIATE_TOKEN = "--font-size-hero:clamp(44px,6vw,76px)"
AGENT_NAMES = ("Claude Code", "Codex", "Pi", "OpenCode")

FEATURES_TITLE_MARKUP = (
    '<h2 class="section-title">From issue<br>'
    '<span class="title-accent">to merge.</span></h2>'
)
FEATURES_LABEL_MARKUP = (
    '<span class="eyebrow eyebrow--muted section-label">HOW IT WORKS /05</span>'
)
FEATURE_CARDS = (
    (
        "/01",
        "ISSUE",
        "Start with the issue",
        "Write what you want. Workshop it with an agent until it's clear enough to build. That conversation is the spec.",
    ),
    (
        "/02",
        "WORKFLOW",
        "The agent plans the work",
        "From the issue, the agent builds a workflow and runs it. Every change lands on one branch in its own worktree, so nothing touches main until you say so.",
    ),
    (
        "/03",
        "REVIEW",
        "Spot checks, not archaeology",
        "A review agent organizes the diff: what changed, why, and where to look first. Comment on any line. Your notes go back as one instruction.",
    ),
    (
        "/04",
        "MERGE",
        "Approve and merge from the same screen",
        "Git is the source of truth, so what you review is exactly what the agent did. Merge, and the branch cleans itself up.",
    ),
    (
        "/05",
        "YOUR MACHINE",
        "Your code stays on your hardware",
        "Agents run locally in your own harness. Check in from your phone. Nothing in the middle can read your repo.",
    ),
)
CARD_SPAN_MODIFIERS = ("", "", "--span-2", "--span-4", "--span-6")
ARTICLE_TAG_PATTERN = r"<article[^>]*>"
SPAN_MODIFIER_PATTERN = r"--span-\d"

ACTIVE_RAIL_DASH_MARKUP = '<i class="rail-dash is-active" data-rail-dash></i>'
IDLE_RAIL_DASH_MARKUP = '<i class="rail-dash" data-rail-dash></i>'
IDLE_RAIL_DASH_COUNT = len(FEATURE_CARDS) - 1

FLOW_SECTION_OPENER = '<section class="flow-section rule-bottom" id="flow">'
FLOW_TITLE_MARKUP = (
    '<h2 class="section-title">A run,<br>'
    '<span class="title-accent">start to finish.</span></h2>'
)
FLOW_LABEL_MARKUP = '<span class="eyebrow eyebrow--muted section-label">EXAMPLE RUN /06</span>'
FLOW_STEPS = (
    ("/01", "YOU", "you", "to-agents", "Open an issue", "Say what you want, in your words."),
    ("/02", "AGENTS", "agents", "", "Workshop it", "The agent asks what's unclear. You answer. The issue is now the spec."),
    ("/03", "AGENTS", "agents", "", "Plan and build", "A workflow runs. Every change lands on one branch in its own worktree."),
    ("/04", "AGENTS", "agents", "to-you", "Organize the diff", "A review agent sorts what changed, why, and where to look first."),
    ("/05", "YOU", "you", "", "Spot-check", "“Ready for review” hits your inbox. Comment on a line, or approve."),
    ("/06", "YOU", "you", "", "Merge", "The branch cleans itself up."),
)
FLOW_STEP_RULES = (
    ".flow-steps{list-style:none;display:grid;gap:var(--flow-row-gap)}",
    ".flow-step{position:relative;background:var(--surface-card);border:var(--hairline);"
    "padding:var(--flow-node-padding);--font-size-card-title:var(--font-size-flow-title);"
    "--line-height-card-title:var(--line-height-flow-title);"
    "--font-size-card-body:var(--font-size-flow-body);"
    "--line-height-card-body:var(--line-height-flow-body);"
    "--space-card-header-bottom:var(--space-flow-header-bottom);"
    "--space-card-title-bottom:var(--space-flow-title-bottom)}",
    '.flow-step::before{content:"";position:absolute;pointer-events:none;'
    "top:100%;left:50%;height:var(--flow-row-gap);border-left:var(--flow-link)}",
    ".flow-step:last-child::before{content:none}",
)
FLOW_DESKTOP_MEDIA_PRELUDE = "@media (min-width:1024px){"
FLOW_DESKTOP_RULES = (
    ".flow-steps{grid-template-columns:repeat(var(--flow-columns),1fr);grid-auto-rows:1fr;"
    "column-gap:var(--flow-col-gap)}",
    ".flow-step--you{grid-row:1}",
    ".flow-step--agents{grid-row:2}",
    ".flow-step::before{top:50%;left:100%;width:var(--flow-col-gap);"
    "height:auto;border-left:0;border-top:var(--flow-link)}",
    ".flow-step--to-agents::before,.flow-step--to-you::before{"
    "width:calc(var(--flow-col-gap) / 2);height:calc(100% + var(--flow-row-gap));"
    "border-right:var(--flow-link)}",
    ".flow-step--to-you::before{top:auto;bottom:50%;border-top:0;border-bottom:var(--flow-link)}",
    ".flow-step--to-agents::after{top:calc(150% + var(--flow-row-gap))}",
    ".flow-step--to-you::after{top:calc(-50% - var(--flow-row-gap))}",
)
FLOW_ROW_GAP_DESKTOP_TOKEN = "--flow-row-gap:28px"
FLOW_ROW_GAP_MOBILE_TOKEN = "--flow-row-gap:18px"

CTA_TITLE_MARKUP = (
    '<h2 class="section-title section-title--cta">Get in '
    '<span class="title-accent title-accent--glow">early.</span></h2>'
)
CTA_NOTE_MARKUP = (
    '<p class="note cta-note">PRIVATE BETA · RUNS ON YOUR MACHINE</p>'
)
CTA_NOTE_RULE = (
    ".cta-note{color:var(--color-text-secondary);"
    "--font-size-note:var(--font-size-cta-note);"
    "--line-height-note:var(--line-height-cta-note);"
    "--letter-spacing-note:var(--letter-spacing-cta-note);"
    "--space-note-top:var(--space-cta-note-top)}"
)
CTA_NOTE_DESKTOP_LINE_HEIGHT = "--line-height-cta-note:normal"
CTA_NOTE_MOBILE_LINE_HEIGHT = "--line-height-cta-note:1.7"
CTA_FORM_SPACING_RULE = "--space-form-top:var(--space-cta-form-top)}"

WAITLIST_ERROR_MARKUP = (
    '<p class="note waitlist-error" role="alert" hidden>'
    "✗ COULDN’T ADD YOU — TRY AGAIN.</p>"
)
WAITLIST_SUCCESS_MARKUP = (
    '<p class="waitlist-success" role="status" hidden>✓ YOU’RE ON THE LIST — '
    "<span data-waitlist-email></span></p>"
)
WAITLIST_CONFIRMATION_MARKUP = (
    '<p class="note waitlist-confirmation" role="status" hidden>'
    "A confirmation is on its way. Every email has a one-click unsubscribe.</p>"
)
DISABLED_BUTTON_RULE = (
    ".button-primary:disabled{cursor:default;box-shadow:none;"
    "opacity:var(--opacity-disabled)}"
)
WAITLIST_CONFIRMATION_RULE = (
    ".waitlist-confirmation{color:var(--color-text-secondary)}"
)
WAITLIST_WRAPPER_RULE = (
    ".waitlist{margin:var(--space-form-top) auto 0;"
    "max-width:var(--waitlist-max-width);width:fit-content}"
)
WAITLIST_WRAPPER_MOBILE_RULE = ".waitlist{margin-inline:0;width:auto}"
WAITLIST_SUBMITTED_RULE = ".waitlist:has(.waitlist-form[hidden]){max-width:none}"
WAITLIST_INSTANCE_COUNT = 2
WAITLIST_PLACEHOLDER = slot_placeholder(WAITLIST_SLOT_NAME)
UNSUBSCRIBE_PANEL_RULES = (
    ".unsubscribe-panel{padding-block:var(--cta-padding-block)}",
    ".unsubscribe-panel .brand{justify-content:center;margin-bottom:var(--space-eyebrow-bottom)}",
    ".unsubscribe-action{margin-top:var(--space-form-top)}",
)
UNSUBSCRIBE_ADDRESS_RULE = (
    ".unsubscribe-address{text-transform:none;overflow-wrap:anywhere}"
)

NAV_ELEMENT_PATTERN = r"<nav\b.*?</nav>"
ANCHOR_HREF_PATTERN = r'href="#([^"]*)"'

CLASS_ATTRIBUTE_PATTERN = r'class="([^"]+)"'
CSS_CLASS_PATTERN = r"\.([A-Za-z][\w-]*)"

MEDIA_BLOCK_OPENER = "@media"
RULE_PATTERN = r"([^{}]+)\{[^{}]*\}"
COMPONENT_LAYER_SELECTORS = (
    ".eyebrow",
    ".eyebrow--muted",
    ".note",
    ".section-header",
    ".section-title",
    ".section-title--hero",
    ".section-title--cta",
    ".title-accent",
    ".title-accent--glow",
    ".rule-top",
    ".rule-bottom",
    ".button-primary",
    ".button-primary--nav",
    ".waitlist",
    ".waitlist-form",
    ".waitlist-input",
    ".waitlist-success",
    ".waitlist-error",
    ".feature-cards",
    ".feature-card",
    ".feature-card--span-2",
    ".feature-card--span-4",
    ".feature-card--span-6",
    ".card-header",
    ".card-number",
    ".card-title",
    ".card-body",
    ".rail-dashes",
    ".agents-grid",
    ".agents-label",
    ".agent-cell",
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


@pytest.mark.parametrize("asset_path, expected_media_type", ASSET_MEDIA_TYPES)
def test_landing_asset_serves_shipped_asset_with_media_type(
    asset_path, expected_media_type
):
    response = RootController.landing_asset.fn(None, asset_path=asset_path)
    assert isinstance(response, Response)
    assert response.media_type == expected_media_type
    assert (LANDING_DIR / asset_path).is_file()


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
    assert "<style" not in html
    for _, script_body in re.findall(SCRIPT_ELEMENT_PATTERN, html, re.S):
        assert script_body.strip() == ""


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
        block_comment_opener = COMMENT_OPENERS_BY_SUFFIX.get(source.suffix)
        if block_comment_opener is None:
            continue
        source_text = source.read_text()
        assert block_comment_opener not in source_text
        for line in source_text.splitlines():
            assert not line.strip().startswith(LINE_COMMENT_OPENER)


def test_index_is_a_body_fragment_and_the_shell_owns_the_document():
    assert HEAD_ELEMENT_OPENER not in _landing_text(LANDING_PAGE_NAME)
    documents = [
        source.name
        for source in sorted(LANDING_DIR.rglob(MARKUP_SUFFIX))
        if DOCTYPE_DECLARATION in source.read_text()
    ]
    assert documents == [SHELL_NAME]


def test_hero_copy_is_verbatim():
    html = _landing_html()
    assert LANDING_TITLE_MARKUP in html
    assert LANDING_DESCRIPTION_MARKUP in html
    assert HERO_EYEBROW_MARKUP in html
    assert HERO_TITLE_MARKUP in html
    assert HERO_LEAD_MARKUP in html
    assert HERO_NOTE_MARKUP in html


def test_hero_is_a_column_that_only_locks_to_the_viewport_on_desktop():
    assert HERO_SECTION_OPENER in _landing_html()
    css = _landing_text(STYLESHEET_NAME)
    for rule in (HERO_RULE, HERO_STAGE_RULE, HERO_COPY_CENTERED_RULE, HERO_LAYERS_RULE):
        assert rule in _rules_outside_media_blocks(css)
    assert HERO_VIEWPORT_LOCK_RULE in css
    assert HERO_VIEWPORT_LOCK_RULE not in _rules_outside_media_blocks(css)


def test_agents_rail_pins_to_the_bottom_of_the_hero_over_a_solid_background():
    html = _landing_html()
    hero_end = html.index("</section>\n<section", html.index(AGENTS_RAIL_MARKUP))
    assert html.index(HERO_SCREENSHOT_OPENER) < html.index(AGENTS_RAIL_MARKUP) < hero_end
    assert AGENTS_RAIL_RULE in _rules_outside_media_blocks(_landing_text(STYLESHEET_NAME))


def test_short_desktop_viewports_put_the_copy_beside_the_screenshot():
    css = _landing_text(STYLESHEET_NAME)
    side_block = css[css.index(HERO_SIDE_MEDIA_PRELUDE):]
    side_block = side_block[: side_block.index("\n}")]
    for rule in HERO_SIDE_RULES:
        assert rule in side_block
        assert rule not in _rules_outside_media_blocks(css)


def test_hero_hosts_the_dot_field_canvas():
    assert DOT_FIELD_MARKUP in _landing_html()


def test_screenshot_serves_the_phone_capture_below_the_mobile_breakpoint():
    assert SCREENSHOT_MARKUP in _landing_html()


def test_screenshot_closes_the_hero_below_the_copy_and_fades_out_at_the_bottom():
    html = _landing_html()
    hero_end = html.index("</section>", html.index(HERO_SECTION_OPENER))
    assert html.index(HERO_NOTE_MARKUP) < html.index(HERO_SCREENSHOT_OPENER) < hero_end
    css = _landing_text(STYLESHEET_NAME)
    for rule in (HERO_SCREENSHOT_RULE, SCREENSHOT_FRAME_RULE, SCREENSHOT_FADE_RULE):
        assert rule in _rules_outside_media_blocks(css)
    assert SCREENSHOT_FADE_TOKEN in css
    assert HERO_SCREENSHOT_MOBILE_RULE in css
    assert HERO_SCREENSHOT_MOBILE_RULE not in _rules_outside_media_blocks(css)


def test_screenshot_gap_and_crop_are_retuned_for_the_phone_capture():
    css = _landing_text(STYLESHEET_NAME)
    for desktop_token, mobile_token in (
        (SCREENSHOT_GAP_DESKTOP_TOKEN, SCREENSHOT_GAP_MOBILE_TOKEN),
        (SCREENSHOT_CROP_DESKTOP_TOKEN, SCREENSHOT_CROP_MOBILE_TOKEN),
    ):
        assert desktop_token in _rules_outside_media_blocks(css)
        assert mobile_token in css
        assert mobile_token not in _rules_outside_media_blocks(css)


def test_every_harness_is_listed_by_name_alone():
    html = _landing_html()
    assert AGENTS_LABEL_MARKUP in html
    for agent_name in AGENT_NAMES:
        assert f'<div class="agent-cell">{agent_name}</div>' in html
    assert "agent-support" not in html


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
            f'<span class="eyebrow--muted">{category}</span></div>' in html
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


def test_flow_section_walks_one_run_between_you_and_your_agents():
    html = _landing_html()
    assert html.index(FEATURES_TITLE_MARKUP) < html.index(FLOW_SECTION_OPENER) < html.index(CTA_TITLE_MARKUP)
    assert FLOW_TITLE_MARKUP in html
    assert FLOW_LABEL_MARKUP in html
    for number, actor, lane, link, title, body in FLOW_STEPS:
        classes = f"flow-step flow-step--{lane}" + (f" flow-step--{link}" if link else "")
        assert (
            f'<li class="{classes}">\n'
            f'      <div class="card-header eyebrow"><span class="card-number">{number}</span>'
            f'<span class="eyebrow--muted">{actor}</span></div>\n'
            f'      <h3 class="card-title">{title}</h3>\n'
            f'      <p class="card-body">{body}</p>' in html
        )
    assert html.count('<li class="flow-step ') == len(FLOW_STEPS)


def test_flow_nodes_stack_on_phones_and_form_two_lanes_on_desktop():
    css = _landing_text(STYLESHEET_NAME)
    for rule in FLOW_STEP_RULES:
        assert rule in _rules_outside_media_blocks(css)
    desktop_block = css[css.index(FLOW_DESKTOP_MEDIA_PRELUDE):]
    desktop_block = desktop_block[: desktop_block.index("\n}")]
    for rule in FLOW_DESKTOP_RULES:
        assert rule in desktop_block
        assert rule not in _rules_outside_media_blocks(css)
    assert FLOW_ROW_GAP_DESKTOP_TOKEN in _rules_outside_media_blocks(css)
    assert FLOW_ROW_GAP_MOBILE_TOKEN in css
    assert FLOW_ROW_GAP_MOBILE_TOKEN not in _rules_outside_media_blocks(css)


def test_cta_section_copy_is_verbatim():
    html = _landing_html()
    assert CTA_TITLE_MARKUP in html
    assert CTA_NOTE_MARKUP in html


def test_waitlist_error_and_success_copy_are_verbatim_and_hidden():
    html = _landing_html()
    assert html.count(WAITLIST_ERROR_MARKUP) == WAITLIST_INSTANCE_COUNT
    assert html.count(WAITLIST_SUCCESS_MARKUP) == WAITLIST_INSTANCE_COUNT


def test_waitlist_confirmation_note_is_verbatim_and_hidden_beside_the_success_line():
    html = _landing_html()
    assert html.count(WAITLIST_CONFIRMATION_MARKUP) == WAITLIST_INSTANCE_COUNT
    assert html.index(WAITLIST_SUCCESS_MARKUP) < html.index(WAITLIST_CONFIRMATION_MARKUP)
    assert WAITLIST_CONFIRMATION_RULE in _rules_outside_media_blocks(
        _landing_text(STYLESHEET_NAME)
    )


def test_the_sending_state_styles_the_disabled_button_from_the_opacity_token():
    assert DISABLED_BUTTON_RULE in _rules_outside_media_blocks(
        _landing_text(STYLESHEET_NAME)
    )


def test_the_unsubscribe_panel_centres_the_wordmark_and_spaces_it_off_the_heading():
    css = _rules_outside_media_blocks(_landing_text(STYLESHEET_NAME))
    for panel_rule in UNSUBSCRIBE_PANEL_RULES:
        assert panel_rule in css


def test_a_long_address_wraps_instead_of_overflowing_the_confirm_heading():
    assert UNSUBSCRIBE_ADDRESS_RULE in _rules_outside_media_blocks(
        _landing_text(STYLESHEET_NAME)
    )


def test_the_waitlist_component_is_written_once_and_rendered_at_both_placeholders():
    page_source = _landing_text(LANDING_PAGE_NAME)
    fragment = _landing_text(WAITLIST_FRAGMENT_NAME).rstrip("\n")
    assert page_source.count(WAITLIST_PLACEHOLDER) == WAITLIST_INSTANCE_COUNT
    assert 'class="waitlist"' not in page_source
    html = _landing_html()
    assert html.count(fragment) == WAITLIST_INSTANCE_COUNT
    assert WAITLIST_PLACEHOLDER not in html


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


def test_page_loads_the_entry_module_as_a_same_origin_es_module():
    html = _landing_html()
    scripts = re.findall(SCRIPT_ELEMENT_PATTERN, html, re.S)
    assert len(scripts) == 1
    assert html.count("<script") == 1
    attributes, script_body = scripts[0]
    assert 'type="module"' in attributes
    assert f'src="/landing/{ENTRY_MODULE_NAME}"' in attributes
    assert script_body == ""
    assert ENTRY_SCRIPT_MARKUP in html


def test_no_module_other_than_the_entry_references_the_document():
    modules = list(LANDING_DIR.rglob(MODULE_SUFFIX))
    assert modules
    for module_path in modules:
        if module_path.name == ENTRY_MODULE_NAME:
            continue
        assert DOCUMENT_GLOBAL not in module_path.read_text()


def test_every_landing_module_is_referenced_from_the_entry_graph():
    modules = list(LANDING_DIR.rglob(MODULE_SUFFIX))
    assert modules
    imported_names = {
        imported
        for module_path in modules
        for imported in re.findall(MODULE_IMPORT_PATTERN, module_path.read_text())
    }
    for module_path in modules:
        assert module_path.name == ENTRY_MODULE_NAME or module_path.name in imported_names


def test_agents_label_shares_the_agent_cell_padding():
    css = _landing_text(STYLESHEET_NAME)
    assert AGENTS_LABEL_RULE in _rules_outside_media_blocks(css)


def test_waitlist_shrinks_to_its_content_and_fills_the_column_on_mobile():
    css = _landing_text(STYLESHEET_NAME)
    assert WAITLIST_WRAPPER_RULE in _rules_outside_media_blocks(css)
    assert WAITLIST_WRAPPER_MOBILE_RULE in css
    assert WAITLIST_WRAPPER_MOBILE_RULE not in _rules_outside_media_blocks(css)


def test_cta_note_repoints_the_note_tokens_at_both_breakpoints():
    css = _landing_text(STYLESHEET_NAME)
    assert CTA_NOTE_RULE in _rules_outside_media_blocks(css)
    assert CTA_NOTE_DESKTOP_LINE_HEIGHT in _rules_outside_media_blocks(css)
    assert CTA_NOTE_MOBILE_LINE_HEIGHT in css
    assert CTA_NOTE_MOBILE_LINE_HEIGHT not in _rules_outside_media_blocks(css)


def test_the_cta_drives_the_waitlist_spacing_through_the_component_token():
    css = _landing_text(STYLESHEET_NAME)
    assert CTA_FORM_SPACING_RULE in _rules_outside_media_blocks(css)
    assert ".cta-section .waitlist" not in css


def test_size_literals_live_only_in_the_token_blocks():
    css = _landing_text(STYLESHEET_NAME)
    remainder = re.sub(MEDIA_PRELUDE_PATTERN, "", re.sub(ROOT_BLOCK_PATTERN, "", css))
    assert re.search(SIZE_LITERAL_PATTERN, remainder) is None


def test_the_hairline_is_one_token_every_rule_reuses():
    remainder = re.sub(ROOT_BLOCK_PATTERN, "", _landing_text(STYLESHEET_NAME))
    assert "1px solid" not in remainder


def test_agents_label_keeps_the_cell_padding_on_mobile():
    css = _landing_text(STYLESHEET_NAME)
    assert AGENTS_LABEL_MOBILE_RULE in css
    assert AGENTS_LABEL_MOBILE_RULE not in _rules_outside_media_blocks(css)


def test_agents_strip_label_column_shrinks_between_the_breakpoints():
    css = _landing_text(STYLESHEET_NAME)
    assert AGENTS_GRID_INTERMEDIATE_RULE in css
    assert AGENTS_GRID_INTERMEDIATE_RULE not in _rules_outside_media_blocks(css)


def test_hero_title_is_pixel_exact_on_desktop_and_scales_only_between_the_breakpoints():
    css = _landing_text(STYLESHEET_NAME)
    assert HERO_FONT_SIZE_DESKTOP_TOKEN in _rules_outside_media_blocks(css)
    assert HERO_FONT_SIZE_INTERMEDIATE_TOKEN in css
    assert HERO_FONT_SIZE_INTERMEDIATE_TOKEN not in _rules_outside_media_blocks(css)


def test_waitlist_success_line_is_not_capped_at_the_form_width():
    assert WAITLIST_SUBMITTED_RULE in _landing_text(STYLESHEET_NAME)


def test_every_markup_class_has_a_rule_and_every_rule_class_is_applied():
    rendered_pages = (
        _landing_html(),
        render_confirm_page(CONFIRM_PAGE_ADDRESS, CONFIRM_PAGE_TOKEN),
        render_removed_page(),
        render_invalid_page(),
    )
    markup_classes = {
        class_name
        for page in rendered_pages
        for attribute in re.findall(CLASS_ATTRIBUTE_PATTERN, page)
        for class_name in attribute.split()
    }
    stylesheet = _landing_text(STYLESHEET_NAME).replace(FONT_FACE_DECLARATION, "")
    styled_classes = set(re.findall(CSS_CLASS_PATTERN, stylesheet))
    module_sources = "".join(
        module_path.read_text() for module_path in LANDING_DIR.rglob(MODULE_SUFFIX)
    )
    assert markup_classes
    assert markup_classes <= styled_classes
    for styled_class in styled_classes - markup_classes:
        assert f'"{styled_class}"' in module_sources


