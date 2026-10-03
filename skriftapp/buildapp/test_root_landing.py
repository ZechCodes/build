"""Behavioural contract for the public landing and its practical exits.

The homepage is a static Astro build (`landing/` at the repo root) emitted into
``landing/generated/``. skriftapp serves that document as it is, filling exactly the
two slots that cannot be static: the activity feed and the repository link. These
checks own that boundary — the document is not re-wrapped, no slot survives into the
response, an unbuilt tree says so plainly, and every same-origin asset is routable.

The page's own composition (acts, copy, choreography) is the landing project's test
suite (`landing/test/`), not Python's.
"""

from __future__ import annotations

import asyncio
import re
from html import escape
from pathlib import Path

import pytest
from litestar.exceptions import NotFoundException
from litestar.response import Response

from buildapp import landing_content, releases
from buildapp.landing_page import LANDING_DIR, SHELL_NAME
from buildapp.landing_content import (
    _render_activity,
    _render_activity_section,
    load_content,
    render_homepage_slots,
)
from buildapp.root_controller import (
    GENERATED_PAGE,
    UNBUILT_LANDING_MESSAGE,
    RootController,
    render_docs_page,
    render_landing_page,
    render_privacy_page,
)

# Imported by the panel/invite/unsubscribe suites. Their pages render through the
# shell; the marketing homepage no longer does, and supplies its own head and footer.
STYLESHEET_LINK = '<link rel="stylesheet" href="/landing/landing.css">'
FONT_PRELOAD_LINK = (
    '<link rel="preload" href="/landing/fonts/JetBrainsMono-latin.woff2" '
    'as="font" type="font/woff2" crossorigin>'
)
FOOTER_ASSURANCE_COPY = "LOCAL-FIRST // E2E ENCRYPTED"
FOOTER_COPYRIGHT_COPY = "© 2026 BUILD · GETBUILD.ING"

CI_WORKFLOW_PATH = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
CANONICAL_REPOSITORY_URL = "https://github.com/ZechCodes/build"

# A stand-in for the Astro output: a complete document carrying both server slots,
# so these checks run on a tree that has never run `npm run build`.
GENERATED_DOCUMENT = (
    "<!doctype html><html lang=\"en\"><head><title>Build</title></head>"
    '<body><main><div class="hero-field notification-wall" data-hero-field aria-hidden="true">'
    '<div data-wall-routine><div data-wall-first-note>Reading auth.ts</div></div>'
    "</div>Your agents. Your machine. Your call.</main>"
    '<div class="practical">{{activity_section}}</div>'
    '<footer><a href="{{repository_url}}">GitHub</a></footer>'
    "</body></html>\n"
)
VERIFIED_ACTIVITY = {
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

# Every device and screen asset the story hands to the browser. Each is a same-origin
# file with a hard size ceiling: the page is the first thing a stranger loads.
ASSET_SIZE_LIMIT = 2_000_000
HOST_ASSETS = (
    "assets/devices/laptop-low.glb",
    "assets/devices/tablet.glb",
    "assets/devices/phone.glb",
    "assets/devices/metadata.json",
    "assets/devices/laptop.webp",
    "assets/devices/tablet.webp",
    "assets/devices/phone.webp",
    *(
        f"assets/devices/scene-0{act}-{profile}.webp"
        for act in range(1, 9)
        for profile in ("desktop", "mobile")
    ),
    "assets/screens/ui10-editor-macbook.webp",
    "assets/screens/ui12-tasks-macbook.webp",
    "assets/screens/ui13-team-macbook.webp",
    "assets/screens/ui14-git-macbook.webp",
    "assets/screens/ui15-triage-ipad.webp",
    "assets/screens/ui16-builder-macbook.webp",
    "assets/screens/ui05-merged-macbook.webp",
    "assets/screens/ui05-merged-ipad.webp",
    "assets/screens/ui05-merged-iphone.webp",
    "assets/screens/ui05-approval-ipad.webp",
    "assets/screens/ui03-question-iphone.webp",
    "assets/screens/ui03-answer-iphone.webp",
    "assets/screens/ui03-resumed-iphone.webp",
)
MEDIA_TYPES = (
    ("cinematic.css", "text/css"),
    ("waitlist-form.js", "text/javascript"),
    ("brand-mark.svg", "image/svg+xml"),
    ("favicon.svg", "image/svg+xml"),
    ("favicon.png", "image/png"),
    ("assets/devices/laptop.webp", "image/webp"),
    ("assets/devices/laptop-low.glb", "model/gltf-binary"),
    ("assets/devices/metadata.json", "application/json"),
    ("content.json", "application/json"),
    ("fonts/inter-400-latin.woff2", "font/woff2"),
)
# The build's own output, served from the same route one directory deeper.
GENERATED_MEDIA_TYPES = (
    ("generated/_astro/index.Dd5lpgnm.js", "text/javascript"),
    ("generated/_astro/index.CngaKg5u.css", "text/css"),
    ("generated/waitlist-boot.js", "text/javascript"),
)


@pytest.fixture
def built_page(monkeypatch, tmp_path) -> Path:
    """A generated document on disk, standing in for `cd landing && npm run build`."""
    page = tmp_path / "generated" / "index.html"
    page.parent.mkdir(parents=True)
    page.write_text(GENERATED_DOCUMENT)
    monkeypatch.setattr("buildapp.root_controller.GENERATED_PAGE", page)
    return page


def test_the_generated_page_is_the_astro_build_output():
    assert GENERATED_PAGE == LANDING_DIR / "generated" / "index.html"


def test_root_serves_the_generated_document_with_only_its_two_slots_filled(built_page):
    response = asyncio.run(RootController.root.fn(None))
    assert isinstance(response, Response)
    # Litestar fills the handler's default status on the way out; only the 503 below
    # sets one on the response itself.
    assert response.status_code is None
    assert response.media_type == "text/html"
    slots = render_homepage_slots()
    assert response.content == GENERATED_DOCUMENT.replace(
        "{{activity_section}}", slots["activity_section"]
    ).replace("{{repository_url}}", slots["repository_url"])
    assert 'class="hero-field notification-wall" data-hero-field aria-hidden="true"' in response.content
    assert "data-wall-first-note>Reading auth.ts" in response.content
    assert "{{" not in response.content


def test_the_generated_document_is_served_whole_and_never_wrapped_in_the_shell(built_page):
    html = render_landing_page()
    assert html.startswith("<!doctype html>")
    assert html.rstrip().endswith("</html>")
    # Astro emits the document; wrapping it in shell.html would nest two of everything.
    assert html.count("<html") == 1
    assert html.count("<head") == 1
    assert html.count("<body") == 1
    assert STYLESHEET_LINK not in html
    assert FOOTER_ASSURANCE_COPY not in html


def test_an_unbuilt_landing_answers_service_unavailable_in_one_plain_sentence(
    monkeypatch, tmp_path
):
    monkeypatch.setattr(
        "buildapp.root_controller.GENERATED_PAGE", tmp_path / "generated" / "index.html"
    )
    response = asyncio.run(RootController.root.fn(None))
    assert response.status_code == 503
    assert response.media_type == "text/plain"
    assert response.content == UNBUILT_LANDING_MESSAGE
    assert UNBUILT_LANDING_MESSAGE == "The landing page has not been built yet."


def test_an_empty_activity_cache_leaves_the_activity_section_off_the_page(built_page):
    content = load_content()
    assert content["source"]["repository_public"] is True
    assert content["activity"]["entries"] == []
    html = render_landing_page()
    # An empty "built in the open" section says less than no section.
    assert 'id="activity"' not in html
    assert content["activity"]["reason"] not in html
    assert "Merged, not necessarily released" not in html


def test_a_verified_activity_entry_renders_into_the_generated_document(
    built_page, monkeypatch
):
    monkeypatch.setattr(
        landing_content,
        "load_content",
        lambda: {
            "source": {"repository_url": CANONICAL_REPOSITORY_URL},
            "activity": VERIFIED_ACTIVITY,
        },
    )
    html = render_landing_page()
    assert 'id="activity"' in html
    assert "Improve connection recovery" in html
    assert "Merged, not necessarily released" in html
    assert "{{" not in html


def test_the_repository_url_is_escaped_into_every_link_it_fills(built_page, monkeypatch):
    monkeypatch.setattr(
        landing_content,
        "load_content",
        lambda: {
            "source": {"repository_url": '/" onmouseover="alert(1)'},
            "activity": {"entries": []},
        },
    )
    html = render_landing_page()
    assert '" onmouseover="' not in html
    assert "&quot; onmouseover=&quot;" in html


def test_the_homepage_asks_for_exactly_two_values_at_request_time():
    slots = render_homepage_slots()
    assert set(slots) == {"activity_section", "repository_url"}
    assert slots["repository_url"] == escape(
        load_content()["source"]["repository_url"], quote=True
    )


def test_the_shipped_content_names_the_canonical_repository():
    source = load_content()["source"]
    assert source["repository_url"] == CANONICAL_REPOSITORY_URL
    assert f"https://github.com/{source['repository']}" == CANONICAL_REPOSITORY_URL


def test_a_small_verified_activity_cache_renders_without_becoming_a_page_dependency():
    activity = {**VERIFIED_ACTIVITY, "entries": [dict(VERIFIED_ACTIVITY["entries"][0])]}
    rendered = _render_activity(activity)
    assert "Improve connection recovery" in rendered
    assert "Merged, not necessarily released" in rendered

    section = _render_activity_section(activity, CANONICAL_REPOSITORY_URL)
    assert 'id="activity"' in section and "Built in the open." in section
    assert "View project history on GitHub" in section

    activity["entries"][0].pop("source_url")
    assert _render_activity(activity) == ""
    assert _render_activity_section(activity, CANONICAL_REPOSITORY_URL) == ""


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
    assert escape(releases.install_command("https://getbuild.ing")) in html
    assert escape(
        releases.install_command("https://getbuild.ing", desktop=True)
    ) in html
    assert (
        "Installer downloads are public. Alpha access is required to pair and use a host."
        in html
    )
    assert "authenticated installer" not in html
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


@pytest.mark.parametrize("asset_path, expected_media_type", GENERATED_MEDIA_TYPES)
def test_the_build_output_is_served_from_the_same_asset_route(
    monkeypatch, tmp_path, asset_path: str, expected_media_type: str
):
    asset = tmp_path / asset_path
    asset.parent.mkdir(parents=True, exist_ok=True)
    asset.write_bytes(b"/* built */")
    monkeypatch.setattr("buildapp.root_controller.LANDING_DIR", tmp_path)
    response = RootController.landing_asset.fn(None, asset_path=asset_path)
    assert response.media_type == expected_media_type
    assert response.content == b"/* built */"


def test_the_generated_document_is_not_offered_as_a_second_homepage(monkeypatch, tmp_path):
    """`/` is the homepage. The same bytes under /landing/ are not a second one, so
    the route hands them back untyped rather than as a page a browser would render."""
    page = tmp_path / "generated" / "index.html"
    page.parent.mkdir(parents=True)
    page.write_text(GENERATED_DOCUMENT)
    monkeypatch.setattr("buildapp.root_controller.LANDING_DIR", tmp_path)
    response = RootController.landing_asset.fn(None, asset_path="generated/index.html")
    assert response.media_type == "application/octet-stream"


def test_every_device_asset_in_the_handoff_exists_and_is_bounded():
    for name in HOST_ASSETS:
        asset = LANDING_DIR / name
        assert asset.is_file(), name
        assert asset.stat().st_size < ASSET_SIZE_LIMIT, name


@pytest.mark.skipif(
    not GENERATED_PAGE.is_file(), reason="the landing project has not been built here"
)
def test_every_same_origin_landing_asset_in_the_built_document_exists():
    paths = set(re.findall(r'(?:src|href)="/landing/([^"?#]+)', render_landing_page()))
    assert paths
    for path in paths:
        assert (LANDING_DIR / path).is_file(), path


def test_landing_assets_reject_traversal_and_missing_files():
    for path in ("../controllers.py", "generated/../../controllers.py", "no-such-asset.js"):
        with pytest.raises(NotFoundException):
            RootController.landing_asset.fn(None, asset_path=path)


def test_only_the_shell_is_a_complete_html_document():
    documents = [
        source.name
        for source in LANDING_DIR.glob("*.html")
        if "<!doctype" in source.read_text()
    ]
    assert documents == [SHELL_NAME]


def test_deploy_smoke_checks_the_new_story_and_critical_assets():
    workflow = CI_WORKFLOW_PATH.read_text()
    for expected in (
        "Build: know what your coding agents need from you",
        "/landing/assets/devices/laptop-low.glb",
        "/landing/assets/screens/ui10-editor-macbook.webp",
        "/landing/assets/devices/scene-01-desktop.webp",
        "/landing/generated/waitlist-boot.js",
    ):
        assert expected in workflow, expected
    for retired in ("/landing/cinematic.css", "/landing/vendor/three-device-runtime.js"):
        assert retired not in workflow, retired
