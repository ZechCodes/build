"""Build's own sign-in and create-account page (#312): ``templates/auth/passkey_login.html``
overrides Skrift's, loads only same-origin files, and its script's WebAuthn ordering is
proved by ``js_tests/passkey-signin.test.mjs`` under node, run from here so this gate
covers it."""

from __future__ import annotations

import re
import shutil
import subprocess

import pytest
from jinja2 import Environment, FileSystemLoader
from litestar import Litestar
from litestar.testing import TestClient
from markupsafe import Markup
from skrift.app_factory import get_template_directories_for_theme

from buildapp.db_test_support import session_backend_config
from buildapp.root_controller import LANDING_DIR, RootController
from buildapp.skrift_app_test_support import SECURE_ORIGIN, SKRIFTAPP_DIR, TEMPLATES_DIR
TEMPLATE_NAME = "auth/passkey_login.html"
#: /auth/login, where /app sends a signed-out visitor first.
LOGIN_TEMPLATE_NAME = "auth/login.html"
SCRIPT_PATH = "/landing/passkey-signin.js"
STYLESHEET_PATH = "/landing/signin.css"
JS_TESTS = SKRIFTAPP_DIR / "js_tests" / "passkey-signin.test.mjs"
NONCE = "test-nonce"


class Descriptor:
    def __init__(
        self,
        *,
        is_available: bool = True,
        availability_note: str | None = None,
        method_type: str = "passkey",
        name: str = "Passkey",
    ):
        self.is_available = is_available
        self.availability_note = availability_note
        self.method_type = method_type
        self.name = name


def render(template_name: str = TEMPLATE_NAME, **context) -> str:
    environment = Environment(  # nosemgrep: python.flask.security.xss.audit.direct-use-of-jinja2.direct-use-of-jinja2
        loader=FileSystemLoader(str(TEMPLATES_DIR)), autoescape=True
    )
    defaults = dict(
        method_key="passkey",
        descriptor=Descriptor(),
        flash=None,
        flash_messages=[],
        invite_email=None,
        csp_nonce=lambda: NONCE,
        csrf_field=lambda: Markup('<input type="hidden" name="_csrf" value="t">'),
    )
    defaults.update(context)
    return environment.get_template(template_name).render(**defaults)


@pytest.mark.parametrize("template_name", [TEMPLATE_NAME, LOGIN_TEMPLATE_NAME])
def test_skrift_finds_our_template_before_its_own(monkeypatch, template_name):
    monkeypatch.chdir(SKRIFTAPP_DIR)
    directories = get_template_directories_for_theme("")
    owner = next(d for d in directories if (d / template_name).is_file())
    assert owner == TEMPLATES_DIR


def test_the_image_ships_the_template_and_the_assets_it_loads():
    containerfile = (SKRIFTAPP_DIR / "Containerfile").read_text()
    assert "COPY skriftapp/templates ./templates" in containerfile
    assert "COPY skriftapp/buildapp ./buildapp" in containerfile
    for asset in (SCRIPT_PATH, STYLESHEET_PATH):
        assert (LANDING_DIR / asset.removeprefix("/landing/")).is_file()


def test_every_url_the_page_loads_is_same_origin():
    html = render()
    loaded = re.findall(r'(?:src|href)="([^"]+)"', html)
    assert loaded
    assert all(url.startswith("/") and not url.startswith("//") for url in loaded), loaded


def test_its_only_script_is_the_same_origin_module_with_the_nonce():
    scripts = re.findall(r"<script[^>]*>", render())
    assert scripts == [f'<script type="module" src="{SCRIPT_PATH}" nonce="{NONCE}">']


def test_it_keeps_skrifts_endpoints_by_naming_the_method():
    assert 'data-passkey-method="passkey"' in render()


INVITED = "invitee@example.com"


def test_the_invite_address_field_has_a_label():
    html = render(invite_email=INVITED)
    assert '<label for="signup-email">' in html
    assert 'id="signup-email"' in html


def test_the_invite_address_is_escaped():
    html = render(invite_email='a"><script>@example.com')
    assert "<script>@" not in html


def test_sign_in_offers_no_autofill_and_no_field():
    html = render()
    assert "webauthn" not in html
    assert 'id="signin-email"' not in html


@pytest.mark.parametrize(
    ("invite_email", "status_ids"),
    [(INVITED, ["signin-status", "signup-status"]), (None, ["signin-status"])],
)
def test_status_lines_are_announced(invite_email, status_ids):
    html = render(invite_email=invite_email)
    for status_id in status_ids:
        match = re.search(rf'<p id="{status_id}"[^>]*>', html)
        assert match and 'aria-live="polite"' in match.group(0) and 'role="status"' in match.group(0)


@pytest.mark.parametrize(("invite_email", "forms"), [(INVITED, 2), (None, 1)])
def test_every_form_carries_a_csrf_field(invite_email, forms):
    assert render(invite_email=invite_email).count('name="_csrf"') == forms


def test_an_unavailable_method_says_so_and_offers_no_forms():
    html = render(descriptor=Descriptor(is_available=False, availability_note="Not here."))
    assert "Not here." in html
    assert "signin-form" not in html and "signup-form" not in html


def test_flash_messages_are_shown_and_escaped():
    html = render(flash="<b>Please log in</b>")
    assert "&lt;b&gt;Please log in&lt;/b&gt;" in html


def test_the_script_orders_its_webauthn_requests():
    node = shutil.which("node")
    assert node, "node is needed to run js_tests/passkey-signin.test.mjs"
    result = subprocess.run(
        [node, "--test", str(JS_TESTS)],
        cwd=SKRIFTAPP_DIR,
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.mark.parametrize("asset", [STYLESHEET_PATH, SCRIPT_PATH, "/landing/favicon.png", "/landing/nope"])
def test_landing_assets_never_rewrite_the_session_cookie(asset):
    """The session lives in the cookie. A page's stylesheet, font or icon that answers
    with a copy of the session it was sent overwrites any CSRF rotation a passkey POST
    made while it was in flight, and the next step fails with invalid_csrf."""
    config = session_backend_config()
    app = Litestar([RootController], middleware=[config.middleware])
    with TestClient(app, session_config=config) as client:
        client.set_session_data({"_csrf": "before"})
        response = client.get(asset)
    assert "set-cookie" not in response.headers


def test_the_login_page_is_the_passkey_page_when_passkeys_are_configured():
    html = render(
        LOGIN_TEMPLATE_NAME,
        providers={"passkey": Descriptor()},
        has_dummy=False,
        method_key=None,
        descriptor=None,
    )
    assert 'data-passkey-method="passkey"' in html
    assert 'id="signin-form"' in html


def test_without_passkeys_the_login_page_lists_providers_in_builds_style():
    html = render(
        LOGIN_TEMPLATE_NAME,
        providers={},
        has_dummy=True,
        method_key=None,
        descriptor=None,
    )
    assert f'href="{STYLESHEET_PATH}"' in html
    assert 'href="/auth/dummy/login"' in html
    assert "<script" not in html


def test_through_the_real_app_landing_assets_set_no_session_cookie(skrift_app):
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        # /auth/login?next= puts the next URL in the session, so there is one to echo.
        signed = client.get("/auth/login?next=/app/").headers.get("set-cookie", "")
        session_cookie = signed.split(";", 1)[0]
        assert session_cookie.startswith("session=")
        headers = {"cookie": session_cookie}
        asset = client.get(STYLESHEET_PATH, headers=headers)
        page = client.get("/docs", headers=headers)
    assert asset.status_code == 200
    assert "set-cookie" not in asset.headers
    # The control: a page through the same stack still writes the session back.
    assert "session=" in page.headers.get("set-cookie", "")


def test_through_the_real_app_login_is_our_passkey_page_with_a_working_nonce(skrift_app):
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        response = client.get("/auth/login")
    assert response.status_code == 200
    assert 'data-passkey-method="passkey"' in response.text
    nonce = re.search(r'<script type="module" src="[^"]+" nonce="([^"]+)">', response.text)
    assert nonce and f"'nonce-{nonce.group(1)}'" in response.headers["content-security-policy"]


def test_through_the_real_app_a_flash_message_reaches_the_page(skrift_app):
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        # Skrift flashes this and redirects to /auth/login when nobody is signed in.
        response = client.get("/auth/passkeys")
    assert response.url.path == "/auth/login"
    assert '<p class="signin-flash" role="status">Please log in to manage passkeys.</p>' in response.text
