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
# the `{{USER0}}` slot and the mount point. The build output is gitignored, so
# a checkout that never built the SPA (a CI runner's) has none of its own.
SHELL = '<!doctype html><div id="app" data-user="{{USER0}}"></div>'


@pytest.fixture(autouse=True)
def built_shell(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    (tmp_path / "index.html").write_text(SHELL)
    monkeypatch.setattr(controllers, "STATIC_DIR", tmp_path)


def _answer(value):
    async def answer(*_args, **_kwargs):
        return value

    return answer


def _shell():
    """The SPA shell as a signed-in member is served it."""
    request = SimpleNamespace(session={SESSION_USER_ID: str(uuid4())})
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
