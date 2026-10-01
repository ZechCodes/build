"""Skrift's remaining auth pages in Build's frame (#313): each override in
``templates/`` extends ``auth/_build_page.html``, keeps Skrift's endpoints, form fields
and CSRF handling, and loads nothing from another origin."""

from __future__ import annotations

import asyncio
import re
import shutil
import subprocess
from datetime import datetime
from types import SimpleNamespace

import pytest
from jinja2 import Environment, FileSystemLoader
from litestar.testing import TestClient
from markupsafe import Markup
from skrift.app_factory import get_template_directories_for_theme
from sqlalchemy.ext.asyncio import create_async_engine

from buildapp import test_passkey_signin_page as signin_page
from buildapp.db_test_support import create_skrift_tables

SKRIFTAPP_DIR = signin_page.SKRIFTAPP_DIR
TEMPLATES_DIR = signin_page.TEMPLATES_DIR
STYLESHEET_PATH = signin_page.STYLESHEET_PATH
SECURE_ORIGIN = signin_page.SECURE_ORIGIN
NONCE = signin_page.NONCE
CSRF_INPUT = '<input type="hidden" name="_csrf" value="t">'
#: The whole app as Skrift builds it, passkeys on and production's CSP.
skrift_app = signin_page.skrift_app

LOGOUT_TEMPLATE = "auth/logout_confirm.html"
ERROR_TEMPLATES = ["error.html", "error-404.html", "error-500.html"]
PASSKEYS_TEMPLATE = "auth/passkeys.html"
VERIFY_PASSKEY_TEMPLATE = "auth/verify_passkey.html"
VERIFY_TEMPLATE = "auth/verify.html"
VERIFY_EMAIL_TEMPLATES = [
    "auth/verify_email_pending.html",
    "auth/verify_email_invalid.html",
    "auth/verify_email_success.html",
]
CEREMONY_SCRIPT = '<script type="module" src="/landing/passkey-ceremony.js" nonce="test-nonce"></script>'
JS_TESTS = SKRIFTAPP_DIR / "js_tests" / "passkey-ceremony.test.mjs"


def render(template_name: str, **context) -> str:
    environment = Environment(  # nosemgrep: python.flask.security.xss.audit.direct-use-of-jinja2.direct-use-of-jinja2
        loader=FileSystemLoader(str(TEMPLATES_DIR)), autoescape=True
    )
    defaults = dict(
        csp_nonce=lambda: NONCE,
        csrf_field=lambda: Markup(CSRF_INPUT),
    )
    defaults.update(context)
    return environment.get_template(template_name).render(**defaults)


def loaded_urls(html: str) -> list[str]:
    return re.findall(r'(?:src|href|action)="([^"]+)"', html)


def assert_in_builds_frame(html: str) -> None:
    assert f'<link rel="stylesheet" href="{STYLESHEET_PATH}">' in html
    assert 'class="signin-bar"' in html and 'class="signin-footer"' in html
    urls = loaded_urls(html)
    assert all(url.startswith("/") and not url.startswith("//") for url in urls), urls
    assert "fonts.googleapis.com" not in html and "skrift/css" not in html


@pytest.mark.parametrize("template_name", [
        LOGOUT_TEMPLATE,
        *ERROR_TEMPLATES,
        PASSKEYS_TEMPLATE,
        VERIFY_PASSKEY_TEMPLATE,
        VERIFY_TEMPLATE,
        *VERIFY_EMAIL_TEMPLATES,
    ])
def test_skrift_finds_our_template_before_its_own(monkeypatch, template_name):
    monkeypatch.chdir(SKRIFTAPP_DIR)
    directories = get_template_directories_for_theme("")
    owner = next(d for d in directories if (d / template_name).is_file())
    assert owner == TEMPLATES_DIR


# --- /auth/logout ----------------------------------------------------------------


def test_logout_is_in_builds_frame_with_no_script():
    html = render(LOGOUT_TEMPLATE)
    assert_in_builds_frame(html)
    assert "<script" not in html
    assert "<title>Log out — Build</title>" in html


def test_logout_keeps_skrifts_form():
    html = render(LOGOUT_TEMPLATE)
    form = re.search(r"<form[^>]*>.*?</form>", html, re.S).group(0)
    assert 'method="post"' in form and 'action="/auth/logout"' in form
    assert CSRF_INPUT in form
    assert re.search(r'<button[^>]*type="submit"[^>]*>Log out</button>', form)
    assert re.search(r'<a[^>]*href="/"[^>]*>Cancel</a>', html)


def test_logout_shows_flash_messages_escaped():
    class Message:
        message = "<b>Invalid request.</b>"

    html = render(LOGOUT_TEMPLATE, flash_messages=[Message()])
    assert "&lt;b&gt;Invalid request.&lt;/b&gt;" in html


def test_through_the_real_app_logout_is_ours_and_its_form_signs_out(skrift_app):
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        page = client.get("/auth/logout")
        token = re.search(r'name="_csrf" value="([^"]+)"', page.text).group(1)
        done = client.post("/auth/logout", data={"_csrf": token}, follow_redirects=False)
    assert page.status_code == 200
    assert_in_builds_frame(page.text)
    # Skrift sends a refused token back to the confirm page; the page's own goes home.
    assert done.status_code in (302, 303) and done.headers["location"] == "/", done.text


def test_through_the_real_app_a_refused_logout_comes_back_with_its_message(skrift_app):
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        client.get("/auth/logout")
        response = client.post("/auth/logout", data={"_csrf": "wrong"})
    assert response.url.path == "/auth/logout"
    assert_in_builds_frame(response.text)


# --- error pages -------------------------------------------------------------------
#
# Skrift renders these from its exception handlers with only status_code, message,
# user, site_name and csrf_field (and, when the app fails to start, hint): no request,
# no csp_nonce. So they carry no script and call nothing else.


def render_error(template_name: str, **context) -> str:
    environment = Environment(  # nosemgrep: python.flask.security.xss.audit.direct-use-of-jinja2.direct-use-of-jinja2
        loader=FileSystemLoader(str(TEMPLATES_DIR)), autoescape=True
    )
    return environment.get_template(template_name).render(**context)


@pytest.mark.parametrize("template_name", ERROR_TEMPLATES)
def test_error_pages_are_in_builds_frame_with_no_script(template_name):
    html = render_error(template_name, status_code=500, message="Nope.")
    assert_in_builds_frame(html)
    assert "<script" not in html
    assert re.search(r'<a class="signin-button" href="/">', html)


def test_the_generic_error_page_shows_the_status_and_message_escaped():
    html = render_error("error.html", status_code=403, message="<b>Forbidden</b>")
    assert "Error 403" in html
    assert "<title>Error 403 — Build</title>" in html
    assert "&lt;b&gt;Forbidden&lt;/b&gt;" in html


def test_the_generic_error_page_has_a_fallback_message_and_shows_a_hint():
    html = render_error("error.html", status_code=500, message="", hint="Set SECRET_KEY.")
    assert "An unexpected error occurred." in html
    assert "Set SECRET_KEY." in html


@pytest.mark.parametrize(
    ("template_name", "title"),
    [("error-404.html", "Page not found — Build"), ("error-500.html", "Something went wrong — Build")],
)
def test_the_specific_error_pages_have_their_own_title(template_name, title):
    assert f"<title>{title}</title>" in render_error(template_name, status_code=0, message="")


def test_through_the_real_app_an_auth_404_is_ours(skrift_app):
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        response = client.get("/auth/nothing/here", headers={"accept": "text/html"})
    assert response.status_code == 404
    assert "<title>Page not found — Build</title>" in response.text
    assert_in_builds_frame(response.text)


def test_through_the_real_app_other_auth_errors_are_ours(skrift_app):
    # Without a second-factor passkey method configured, /auth/verify/<key>/options
    # has no GET: Skrift answers 405 through error.html.
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        response = client.get("/auth/verify/passkey/options", headers={"accept": "text/html"})
    assert response.status_code == 405
    assert "<title>Error 405 — Build</title>" in response.text
    assert_in_builds_frame(response.text)


def test_through_the_real_app_an_api_client_still_gets_json(skrift_app):
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        response = client.get("/auth/nothing/here", headers={"accept": "application/json"})
    assert response.status_code == 404
    assert response.json()["status_code"] == 404


# --- /auth/passkeys and /auth/verify/<key>: the pages that run a ceremony ---------


def enrollment(name, *, last_used=None):
    return SimpleNamespace(
        id="7d3f2c4e-0000-4000-8000-000000000001",
        display_name=name,
        enrolled_at=datetime(2026, 10, 1, 9, 30),
        last_used_at=last_used,
    )


def render_passkeys(**context):
    defaults = dict(
        user=SimpleNamespace(email="a@example.com", name="A"),
        factor_key="passkey",
        descriptor=signin_page.Descriptor(),
        enrollments=[],
        flash_messages=[],
    )
    defaults.update(context)
    return render(PASSKEYS_TEMPLATE, **defaults)


def render_verify_passkey(**context):
    defaults = dict(
        descriptor=signin_page.Descriptor(name="Passkey"),
        factor_key="passkey",
        pending_auth=SimpleNamespace(email="a@example.com", name=None),
        flash_messages=[],
    )
    defaults.update(context)
    return render(VERIFY_PASSKEY_TEMPLATE, **defaults)


@pytest.mark.parametrize("render_page", [render_passkeys, render_verify_passkey], ids=["passkeys", "verify"])
def test_ceremony_pages_load_only_the_shared_module_with_the_nonce(render_page):
    html = render_page()
    assert_in_builds_frame(html)
    assert re.findall(r"<script[^>]*>.*?</script>", html, re.S) == [CEREMONY_SCRIPT]


def test_the_ceremony_module_reuses_the_sign_in_codecs():
    script = (signin_page.LANDING_DIR / "passkey-ceremony.js").read_text()
    assert 'from "./passkey-signin.js"' in script


def test_the_ceremony_module_orders_its_webauthn_requests():
    node = shutil.which("node")
    assert node, "node is needed to run js_tests/passkey-ceremony.test.mjs"
    result = subprocess.run(
        [node, "--test", str(JS_TESTS)], cwd=SKRIFTAPP_DIR, capture_output=True, text=True, timeout=120
    )
    assert result.returncode == 0, result.stdout + result.stderr


def test_passkeys_keeps_skrifts_registration_form():
    html = render_passkeys()
    form = re.search(r'<form id="passkey-registration-form"[^>]*>.*?</form>', html, re.S).group(0)
    assert CSRF_INPUT in form
    assert re.search(r'<input[^>]*name="display_name"', form)
    assert re.search(r'<label for="passkey-name">', form)
    status = re.search(r'<p id="passkey-status"[^>]*>', html).group(0)
    assert 'role="status"' in status and 'aria-live="polite"' in status
    assert "a@example.com" in html


def test_passkeys_lists_each_enrollment_with_skrifts_remove_form():
    html = render_passkeys(
        enrollments=[enrollment("<i>Laptop</i>", last_used=datetime(2026, 10, 1, 10, 0)), enrollment(None)]
    )
    assert "&lt;i&gt;Laptop&lt;/i&gt;" in html
    assert "Unnamed passkey" in html
    assert "2026-10-01 09:30" in html and "2026-10-01 10:00" in html
    removes = re.findall(r'<form method="post" action="(/auth/passkeys/[^"]+/delete)">(.*?)</form>', html, re.S)
    assert [action for action, _ in removes] == ["/auth/passkeys/7d3f2c4e-0000-4000-8000-000000000001/delete"] * 2
    assert all(CSRF_INPUT in body for _, body in removes)
    assert "only passkey" not in html


def test_passkeys_warns_before_the_last_one_is_removed():
    assert "only passkey" in render_passkeys(enrollments=[enrollment("Laptop")])


def test_passkeys_with_none_says_so():
    assert "No passkeys yet." in render_passkeys()


def test_passkeys_unavailable_offers_no_form_and_no_script():
    html = render_passkeys(descriptor=signin_page.Descriptor(is_available=False, availability_note="Not here."))
    assert "Not here." in html
    assert "passkey-registration-form" not in html and "<script" not in html


def test_verify_passkey_names_the_factor_for_the_module():
    html = render_verify_passkey(factor_key="security-key")
    form = re.search(r'<form id="passkey-verify-form"[^>]*>.*?</form>', html, re.S).group(0)
    assert 'data-factor-key="security-key"' in form
    assert CSRF_INPUT in form
    assert "a@example.com" in html
    status = re.search(r'<p id="verify-status"[^>]*>', html).group(0)
    assert 'role="status"' in status and 'aria-live="polite"' in status


def test_verify_lists_skrifts_methods():
    methods = [SimpleNamespace(name="Passkey", factor_type="passkey_auth", verify_path="/auth/verify/passkey")]
    html = render(
        VERIFY_TEMPLATE,
        methods=methods,
        pending_auth=SimpleNamespace(email=None, name="<b>A</b>"),
        flash_messages=[],
    )
    assert_in_builds_frame(html)
    assert "<script" not in html
    assert 'href="/auth/verify/passkey"' in html
    assert "Continue with Passkey" in html
    assert "&lt;b&gt;A&lt;/b&gt;" in html


@pytest.mark.parametrize("template_name", VERIFY_EMAIL_TEMPLATES)
def test_verify_email_pages_are_in_builds_frame_with_no_script(template_name):
    html = render(template_name, masked_email="a***@example.com")
    assert_in_builds_frame(html)
    assert "<script" not in html


def test_verify_email_pending_shows_the_masked_address():
    assert "a***@example.com" in render(VERIFY_EMAIL_TEMPLATES[0], masked_email="a***@example.com")
    assert "We sent you a confirmation link." in render(VERIFY_EMAIL_TEMPLATES[0], masked_email=None)


#: auth.second_factors with a passkey method, which is what /auth/passkeys and
#: /auth/verify/<key> serve; production has none, so they are 404 there today.
SECOND_FACTOR_CONFIG = (
    "auth:\n"
    "  second_factors:\n"
    "    enabled: true\n"
    "    methods:\n"
    "      passkey:\n"
    "        type: passkey\n"
)


@pytest.fixture()
def second_factor_app(tmp_path, monkeypatch):
    """The dev app (dummy sign-in) with a passkey second factor, production's CSP and
    nonce, over a throwaway database with Skrift's schema in it."""
    from skrift.asgi import create_app
    from skrift.config import get_settings

    config = (SKRIFTAPP_DIR / "app.dev.yaml").read_text()
    config = config.replace("  csp_nonce: false", "  csp_nonce: true").replace("auth:\n", SECOND_FACTOR_CONFIG, 1)
    production_csp = re.search(r"  content_security_policy: .*", signin_page.PRODUCTION_CONFIG).group(0)
    config = re.sub(r"  content_security_policy: .*", lambda _: production_csp, config)
    database = tmp_path / "app.db"
    (tmp_path / "app.dev.yaml").write_text(config.replace("./app.db", str(database)))
    (tmp_path / "templates").symlink_to(TEMPLATES_DIR)

    async def create_schema() -> None:
        engine = create_async_engine(f"sqlite+aiosqlite:///{database}")
        await create_skrift_tables(engine)
        await engine.dispose()

    asyncio.run(create_schema())
    monkeypatch.setenv("SKRIFT_ENV", "dev")
    monkeypatch.setenv("SECRET_KEY", "a-test-secret-that-is-long-enough-to-use")
    monkeypatch.chdir(tmp_path)
    get_settings.cache_clear()
    try:
        yield create_app()
    finally:
        get_settings.cache_clear()


def csrf_token(html: str) -> str:
    return re.search(r'name="_csrf" value="([^"]+)"', html).group(1)


def sign_in_with_the_dummy(client: TestClient) -> None:
    form = client.get("/auth/dummy/login")
    signed_in = client.post(
        "/auth/dummy-login", data={"_csrf": csrf_token(form.text), "email": "a@example.com"}, follow_redirects=False
    )
    assert signed_in.status_code in (302, 303), signed_in.text


def test_through_the_real_app_manage_passkeys_is_ours_with_a_working_nonce(second_factor_app):
    with TestClient(second_factor_app, base_url=SECURE_ORIGIN) as client:
        sign_in_with_the_dummy(client)
        page = client.get("/auth/passkeys")
    assert page.status_code == 200, page.text
    assert_in_builds_frame(page.text)
    assert 'id="passkey-registration-form"' in page.text
    nonce = re.search(r'<script type="module" src="/landing/passkey-ceremony.js" nonce="([^"]+)">', page.text)
    assert nonce and f"'nonce-{nonce.group(1)}'" in page.headers["content-security-policy"]


def test_through_the_real_app_the_pages_token_starts_a_registration(second_factor_app):
    with TestClient(second_factor_app, base_url=SECURE_ORIGIN) as client:
        sign_in_with_the_dummy(client)
        page = client.get("/auth/passkeys")
        options = client.post("/auth/passkeys/options", data={"_csrf": csrf_token(page.text), "display_name": "x"})
        # A refusal after the check carries no token; the page's re-read finds the new one.
        refused = client.post("/auth/passkeys/complete", data={"_csrf": options.json()["csrf_token"]})
        reread = client.get("/auth/passkeys")
    assert options.is_success, options.text
    assert options.json()["options"]["challenge"]
    assert refused.status_code == 400 and "csrf_token" not in refused.json()
    assert csrf_token(reread.text) not in (csrf_token(page.text), options.json()["csrf_token"])


def test_through_the_real_app_a_removal_flash_reaches_the_page(second_factor_app):
    with TestClient(second_factor_app, base_url=SECURE_ORIGIN) as client:
        sign_in_with_the_dummy(client)
        client.get("/auth/passkeys")
        response = client.post(
            "/auth/passkeys/7d3f2c4e-0000-4000-8000-000000000001/delete", data={"_csrf": "stale"}
        )
    assert response.url.path == "/auth/passkeys"
    assert '<p class="signin-flash" role="status">Your session expired. Please try again.</p>' in response.text


def test_through_the_real_app_an_invalid_email_link_is_ours(skrift_app):
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        response = client.get("/auth/verify-email/claim/not-a-token")
    assert response.status_code == 200
    assert "This link can" in response.text
    assert_in_builds_frame(response.text)
