"""Real primary-login fixtures shared by hosted HTTP authorization tests."""

import re
from uuid import UUID

from litestar.testing import TestClient
from skrift.auth.session_keys import SESSION_USER_ID

from buildapp import skrift_app_test_support as framework


def build_login_app(tmp_path, monkeypatch):
    # Dummy is development-only, but invokes Skrift's actual session rotation,
    # primary-auth completion and after_login hook, as production passkeys do.
    monkeypatch.setattr(
        framework,
        "DEV_CONFIG_EDITS",
        {
            "  csp_nonce: false": "  csp_nonce: true",
            "public_base_url: http://localhost:8090": f"public_base_url: {framework.SECURE_ORIGIN}",
            "redirect_base_url: http://localhost:8080": f"redirect_base_url: {framework.SECURE_ORIGIN}",
        },
    )
    yield from framework.build_skrift_app(tmp_path, monkeypatch)


def framework_client(app, **options):
    from skrift.app_factory import create_session_config
    from skrift.config import get_settings

    settings = get_settings()
    config = create_session_config(
        settings.secret_key,
        max_age=settings.session.max_age,
        secure=not settings.debug,
        cookie_domain=settings.session.cookie_domain,
        cookie_name=settings.session.cookie_name,
    )
    return TestClient(
        app, base_url=framework.SECURE_ORIGIN, session_config=config, **options
    )


def csrf(page):
    match = re.search(r'name="_csrf" value="([^"]+)"', page.text)
    assert match is not None, page.text
    return match.group(1)


def cookie_snapshot(client):
    return [
        (cookie.name, cookie.value, cookie.domain, cookie.path)
        for cookie in client.cookies.jar
    ]


def restore(client, cookies):
    client.cookies.clear()
    for name, value, domain, path in cookies:
        client.cookies.set(name, value, domain=domain, path=path)


def real_login(client, email="login-replay@example.com"):
    client.cookies.clear()
    page = client.get("/auth/dummy/login", follow_redirects=False)
    assert page.status_code == 200, page.text
    result = client.post(
        "/auth/dummy-login",
        data={"_csrf": csrf(page), "email": email, "name": "Bridge HTTP regression"},
        follow_redirects=False,
    )
    assert result.status_code == 302, result.text
    # Save the earliest signed authenticated cookie without following redirects.
    return UUID(client.get_session_data()[SESSION_USER_ID]), cookie_snapshot(client)
