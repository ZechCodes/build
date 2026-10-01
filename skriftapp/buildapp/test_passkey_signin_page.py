"""Build's own sign-in and create-account page (#312): ``templates/auth/passkey_login.html``
overrides Skrift's, loads only same-origin files, and its script's WebAuthn ordering is
proved by ``js_tests/passkey-signin.test.mjs`` under node, run from here so this gate
covers it."""

from __future__ import annotations

import re
import shutil
import subprocess
from pathlib import Path

import pytest
from jinja2 import Environment, FileSystemLoader
from litestar import Litestar
from litestar.testing import TestClient
from markupsafe import Markup
from skrift.app_factory import get_template_directories_for_theme

from buildapp.db_test_support import session_backend_config
from buildapp.root_controller import LANDING_DIR, RootController

SKRIFTAPP_DIR = Path(__file__).resolve().parents[1]
TEMPLATES_DIR = SKRIFTAPP_DIR / "templates"
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


@pytest.mark.parametrize("field_id", ["signin-email", "signup-email", "signup-name"])
def test_every_field_has_a_label(field_id):
    html = render()
    assert f'<label for="{field_id}">' in html
    assert f'id="{field_id}"' in html


def test_autofill_is_offered_on_the_sign_in_email():
    assert 'autocomplete="username webauthn"' in render()


@pytest.mark.parametrize("status_id", ["signin-status", "signup-status"])
def test_status_lines_are_announced(status_id):
    match = re.search(rf'<p id="{status_id}"[^>]*>', render())
    assert match and 'aria-live="polite"' in match.group(0) and 'role="status"' in match.group(0)


def test_both_forms_carry_a_csrf_field():
    assert render().count('name="_csrf"') == 2


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
    assert 'id="signup-form"' in html


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
