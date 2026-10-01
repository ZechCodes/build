"""Skrift's remaining auth pages in Build's frame (#313): each override in
``templates/`` extends ``auth/_build_page.html``, keeps Skrift's endpoints, form fields
and CSRF handling, and loads nothing from another origin."""

from __future__ import annotations

import re

import pytest
from jinja2 import Environment, FileSystemLoader
from litestar.testing import TestClient
from markupsafe import Markup
from skrift.app_factory import get_template_directories_for_theme

from buildapp import test_passkey_signin_page as signin_page

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


@pytest.mark.parametrize("template_name", [LOGOUT_TEMPLATE, *ERROR_TEMPLATES])
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
