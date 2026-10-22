"""The app shell is the one document that says which bundle to load, and it
must never be served from a cache.

Every asset under ``static/assets`` is content-hashed and served immutable, so
the only way a browser moves to a new build is by fetching this HTML again. Sent
with no cache headers at all — no ``Cache-Control``, no ``ETag``, no
``Last-Modified`` — it is the one response a cache is free to make its own
decision about, and a phone that decided to keep it would run a bundle deploys
no longer serve, with nothing in the app able to say so.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from uuid import uuid4

import pytest
from skrift.auth.session_keys import SESSION_USER_ID

from buildapp import controllers
from buildapp.controllers import BuildController


# What `npm run build` in spa/ writes as the shell, cut to the parts under test:
# the account and `{{USER0}}` slots and the mount point. The build output is gitignored, so
# a checkout that never built the SPA (a CI runner's) has none of its own.
SHELL = (
    '<!doctype html><meta name="build-account-id" content="{{ACCOUNT_ID}}">'
    '<div id="app" data-user="{{USER0}}"></div>'
    '<!-- {{ACCOUNT_ID_EXTRA}} -->'
)


@pytest.fixture(autouse=True)
def built_shell(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    (tmp_path / "index.html").write_text(SHELL)
    monkeypatch.setattr(controllers, "STATIC_DIR", tmp_path)


def _answer(value):
    async def answer(*_args, **_kwargs):
        return value

    return answer


def _shell(user_id=None, **request_fields):
    """The SPA shell as a signed-in member is served it."""
    request = SimpleNamespace(
        session={SESSION_USER_ID: str(user_id or uuid4())}, **request_fields
    )
    with patch.object(controllers, "is_alpha_member", _answer(True)), patch.object(
        controllers, "account_email", _answer("someone@example.com")
    ):
        # The class stands in for the instance: the shell is rendered through
        # `self._render_spa`, which the redirect-path tests never reach.
        return asyncio.run(
            BuildController.index.fn(BuildController, request=request, db_session=None)
        )


def test_the_app_shell_is_never_cached():
    assert _shell().headers["Cache-Control"] == "no-store"


def test_the_shell_still_carries_the_app():
    response = _shell()
    assert response.media_type == "text/html"
    assert '<div id="app" data-user="S">' in response.content


def test_the_shell_names_only_the_authenticated_account():
    own_id, other_id = uuid4(), uuid4()
    response = _shell(
        own_id,
        query_params={"account_id": str(other_id)},
        headers={"X-Build-Account-Id": str(other_id)},
        scope={"state": {"build_user_id": str(other_id)}},
    )

    assert f'<meta name="build-account-id" content="{own_id}">' in response.content
    assert str(other_id) not in response.content
    assert "{{ACCOUNT_ID}}" not in response.content
    assert "{{ACCOUNT_ID_EXTRA}}" in response.content


@pytest.mark.parametrize("session", [{}, {SESSION_USER_ID: "not-a-uuid"}])
def test_a_signed_out_request_carries_no_account_id(session):
    other_id = uuid4()
    request = SimpleNamespace(
        session=session, query_params={"account_id": str(other_id)}
    )
    response = asyncio.run(
        BuildController.index.fn(BuildController, request=request, db_session=None)
    )

    assert response.status_code == 302
    assert "build-account-id" not in str(response.content)
    assert str(other_id) not in str(response.content)


def test_the_account_placeholder_is_encoded_as_an_html_attribute():
    account_id = '"<>&\''
    with patch.object(controllers, "account_email", _answer("someone@example.com")):
        response = asyncio.run(BuildController._render_spa(account_id, None))

    assert (
        '<meta name="build-account-id" content="&quot;&lt;&gt;&amp;&#x27;">'
        in response.content
    )


def test_the_spa_source_declares_the_account_placeholder():
    source = Path(__file__).resolve().parents[2] / "spa" / "index.html"
    assert '<meta name="build-account-id" content="{{ACCOUNT_ID}}"' in source.read_text()
