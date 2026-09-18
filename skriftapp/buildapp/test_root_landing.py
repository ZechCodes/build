"""Behavioural contract for the public landing and its practical exits.

The page may change composition and rendering technique without rewriting a screenshot
spec in Python. These checks keep the six approved ideas readable, every production
action real, and every same-origin asset routable.
"""

from __future__ import annotations

import asyncio
import re
from pathlib import Path

import pytest
from litestar.exceptions import NotFoundException
from litestar.response import Response

from buildapp import releases
from buildapp.landing_page import LANDING_DIR, SHELL_NAME, read_landing_file
from buildapp.landing_content import _render_activity, load_content
from buildapp.root_controller import (
    LANDING_PAGE_NAME,
    RootController,
    render_docs_page,
    render_landing_page,
    render_privacy_page,
)

# Imported by the panel/invite/unsubscribe suites. Their pages intentionally retain
# the legacy shell footer while the marketing landing supplies its own compact footer.
STYLESHEET_LINK = '<link rel="stylesheet" href="/landing/landing.css">'
FONT_PRELOAD_LINK = (
    '<link rel="preload" href="/landing/fonts/JetBrainsMono-latin.woff2" '
    'as="font" type="font/woff2" crossorigin>'
)
FOOTER_ASSURANCE_COPY = "LOCAL-FIRST // E2E ENCRYPTED"
FOOTER_COPYRIGHT_COPY = "© 2026 BUILD · GETBUILD.ING"

CI_WORKFLOW_PATH = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
CANONICAL_REPOSITORY_URL = "https://github.com/ZechCodes/build-web"
SCENES = (
    ("start", "Set the work in motion.", "Your coding agents. Your hardware. Any screen."),
    ("handoff", "Your day moves.", "Pick up the same workspace wherever you are."),
    (
        "direction",
        "A little direction. Back to work.",
        "Answer your agents from the screen at hand.",
    ),
    (
        "overview",
        "See the whole picture.",
        "Follow agents, workflows, and shells in one place.",
    ),
    (
        "review",
        "Keep the final say.",
        "Understand the changes. Decide what lands.",
    ),
    ("download", "Any screen. Your call.", "Free &amp; open source."),
)
HOST_ASSETS = (
    "assets/devices/laptop.webp",
    "assets/devices/tablet.webp",
    "assets/devices/phone.webp",
    "assets/devices/laptop.glb",
    "assets/devices/tablet.glb",
    "assets/devices/phone.glb",
    "assets/devices/metadata.json",
)
MEDIA_TYPES = (
    ("cinematic.css", "text/css"),
    ("main.js", "text/javascript"),
    ("brand-mark.svg", "image/svg+xml"),
    ("favicon.svg", "image/svg+xml"),
    ("favicon.png", "image/png"),
    ("assets/devices/laptop.webp", "image/webp"),
    ("assets/devices/laptop.glb", "model/gltf-binary"),
    ("assets/devices/metadata.json", "application/json"),
    ("content.json", "application/json"),
    ("fonts/inter-400-latin.woff2", "font/woff2"),
)


def _landing_html() -> str:
    return render_landing_page()


def test_root_serves_the_complete_landing_document():
    response = asyncio.run(RootController.root.fn(None))
    assert isinstance(response, Response)
    assert response.media_type == "text/html"
    assert response.content == _landing_html()
    assert response.content.startswith("<!doctype html>")
    assert "{{" not in response.content


def test_story_contains_the_six_approved_scenes_in_order():
    html = _landing_html()
    positions = []
    for scene_id, headline, supporting_text in SCENES:
        marker = f'data-story-scene="{scene_id}"'
        assert html.count(marker) == 1
        scene_position = html.index(marker)
        assert html.index(headline, scene_position) > scene_position
        assert supporting_text in html
        positions.append(scene_position)
    assert positions == sorted(positions)


def test_story_actions_reach_real_public_exits():
    html = _landing_html()
    assert html.count('href="#download"') >= 3
    assert html.count(f'href="{CANONICAL_REPOSITORY_URL}"') >= 3
    assert 'href="/docs"' in html
    assert 'href="/privacy"' in html
    assert releases.DOWNLOADS_PATH not in html


def test_download_section_hands_off_to_the_authenticated_alpha_setup():
    html = _landing_html()
    assert '<section class="download-chooser" id="download"' in html
    assert 'href="/app/">Open installer setup</a>' in html
    assert "after alpha access is confirmed" in html
    for _key, label in releases.PLATFORMS:
        assert label in html


def test_mobile_is_described_as_a_client_instead_of_a_binary_target():
    html = _landing_html()
    assert "Mobile devices connect to that host" in html
    assert "iPhone" not in html
    assert "Android" not in html


def test_practical_content_links_to_the_maintained_facts():
    html = _landing_html()
    assert 'href="/docs#harnesses"' in html
    assert "Claude Code, Codex, and Pi are available now; OpenCode is planned." in html
    assert 'href="/privacy"' in html
    assert "content-free connection diagnostics" in html
    assert "optional browser push subscription" in html


def test_private_repository_renders_an_honest_activity_empty_state():
    html = _landing_html()
    content = load_content()
    assert content["source"]["repository_public"] is False
    assert content["activity"]["entries"] == []
    assert content["activity"]["reason"] in html
    assert "View project history on GitHub" in html
    assert "Public source checked" not in html
    assert "Merged, not necessarily released" not in html


def test_a_small_verified_activity_cache_renders_without_becoming_a_page_dependency():
    activity = {
        "reason": "Public development updates will appear here when available.",
        "entries": [
            {
                "summary": "Improve connection recovery",
                "category": "Reliability",
                "merged_at": "2026-09-17T21:10:00Z",
                "source_url": f"{CANONICAL_REPOSITORY_URL}/pull/42",
                "release_status": "merged_not_released",
            }
        ],
    }
    rendered = _render_activity(activity)
    assert "Improve connection recovery" in rendered
    assert "Merged, not necessarily released" in rendered

    activity["entries"][0].pop("source_url")
    assert activity["reason"] in _render_activity(activity)


def test_footer_has_the_five_required_exits():
    html = _landing_html()
    footer = html[html.index('<footer class="landing-footer">') :]
    for label in ("Build", "Docs", "GitHub", "Privacy", "Alpha"):
        assert f">{label}<" in footer


def test_docs_publish_current_host_and_harness_status():
    html = render_docs_page()
    for copy in (
        "macOS or Linux computer",
        "Claude Code",
        "Codex",
        "Pi",
        "OpenCode",
        "Not currently available",
    ):
        assert copy in html
    assert 'id="architecture"' in html
    assert 'href="/app/"' in html
    assert releases.DOWNLOADS_PATH not in html


def test_privacy_page_names_each_real_connection_party_and_provider_boundary():
    html = render_privacy_page()
    for copy in (
        "device records",
        "content-free transport diagnostics",
        "session, account, and device identifiers",
        "remain as transport history",
        "push endpoint",
        "p256dh and auth encryption keys",
        "WebRTC",
        "Cloudflare TURN",
        "source IP",
        "hosted Build service does not receive or store your coding-harness credentials",
        "provider terms and data handling still apply",
    ):
        assert copy in html


@pytest.mark.parametrize("asset_path, expected_media_type", MEDIA_TYPES)
def test_landing_asset_serves_shipped_media_with_an_explicit_type(
    asset_path: str, expected_media_type: str
):
    response = RootController.landing_asset.fn(None, asset_path=asset_path)
    assert response.media_type == expected_media_type
    assert (LANDING_DIR / asset_path).is_file()


def test_every_device_asset_in_the_handoff_exists_and_is_bounded():
    for name in HOST_ASSETS:
        asset = LANDING_DIR / name
        assert asset.is_file()
        assert asset.stat().st_size < 2_000_000


def test_every_same_origin_landing_asset_in_the_document_exists():
    paths = set(re.findall(r'(?:src|href)="/landing/([^"?#]+)', _landing_html()))
    assert paths
    for path in paths:
        assert (LANDING_DIR / path).is_file(), path


def test_landing_assets_reject_traversal_and_missing_files():
    for path in ("../controllers.py", "no-such-asset.js"):
        with pytest.raises(NotFoundException):
            RootController.landing_asset.fn(None, asset_path=path)


def test_only_the_shell_is_a_complete_html_document():
    assert "<head" not in read_landing_file(LANDING_PAGE_NAME)
    documents = [
        source.name
        for source in LANDING_DIR.glob("*.html")
        if "<!doctype" in source.read_text()
    ]
    assert documents == [SHELL_NAME]


def test_page_uses_semantic_content_and_a_single_external_module():
    html = _landing_html()
    assert html.count("<main") == 1
    assert '<nav class="cinematic-nav"' in html
    assert html.count("<script") == 1
    assert '<script type="module" src="/landing/main.js"></script>' in html
    assert "<style" not in html


def test_reduced_motion_keeps_the_story_readable():
    css = read_landing_file("cinematic.css")
    assert "prefers-reduced-motion:reduce" in css.replace(" ", "")
    assert ".story-scene" in css


def test_deploy_smoke_checks_the_new_story_and_critical_assets():
    workflow = CI_WORKFLOW_PATH.read_text()
    for expected in (
        "Set the work in motion",
        "/landing/cinematic.css",
        "/landing/assets/devices/laptop.webp",
    ):
        assert expected in workflow
