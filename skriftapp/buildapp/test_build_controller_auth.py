"""BuildController's auth must ride the shared session helper (no inline
``request.session.get("user_id")`` string literals), redirect anonymous visitors
to login, and treat a malformed session value as "not logged in" — never a 500.

Also pins the sync_to_thread discipline: ``sync_to_thread`` only applies to sync
callables, so no async handler may carry it (it warns on every boot and offloads
nothing)."""

from __future__ import annotations

import asyncio
import inspect
from types import SimpleNamespace
from unittest.mock import patch
from uuid import uuid4

from litestar import Litestar
from litestar.handlers import HTTPRouteHandler
from litestar.response import Redirect

from skrift.auth.session_keys import SESSION_USER_ID

from buildapp import controllers, email_message
from buildapp.controllers import BuildController
from buildapp.desktop_auth import build_auth_guard
from buildapp.email_message import provide_public_base_url
from buildapp.email_test_support import email_settings
from buildapp.invite_pages import INVITE_ONLY_HEADING
from buildapp.releases import PLATFORMS

SIGNED_IN_ADDRESS = "someone@example.com"


def _route_handlers() -> list[HTTPRouteHandler]:
    return [
        handler
        for handler in vars(BuildController).values()
        if isinstance(handler, HTTPRouteHandler)
    ]


def _call_index(session: dict, *, member: bool = False, email: str = SIGNED_IN_ADDRESS):
    request = SimpleNamespace(session=session)
    with patch.object(controllers, "is_alpha_member", _answer(member)), patch.object(
        controllers, "account_email", _answer(email)
    ):
        return asyncio.run(
            BuildController.index.fn(None, request=request, db_session=None)
        )


def _answer(value):
    async def answer(*_args, **_kwargs):
        return value

    return answer


def test_index_redirects_anonymous_visitors_to_login():
    response = _call_index({})
    assert isinstance(response, Redirect)


def test_index_redirects_on_a_malformed_session_user_id_instead_of_500():
    response = _call_index({SESSION_USER_ID: "not-a-uuid"})
    assert isinstance(response, Redirect)


def test_index_reads_the_session_through_the_shared_helper():
    source = inspect.getsource(BuildController.index.fn)
    assert 'session.get("user_id")' not in source, "inline session auth is forbidden"
    assert "session_user_id" in source


def test_desktop_shell_uses_the_reusable_oauth_guard():
    desktop = next(
        handler for handler in _route_handlers() if "/desktop" in handler.paths
    )
    assert build_auth_guard in (desktop.guards or [])


def test_no_async_handler_carries_sync_to_thread():
    for handler in _route_handlers():
        if asyncio.iscoroutinefunction(handler.fn):
            assert handler.sync_to_thread is None, (
                f"{list(handler.paths)}: sync_to_thread has no effect on an async "
                "callable and warns on every boot"
            )


def test_a_signed_in_account_with_no_invite_gets_the_invite_only_page():
    response = _call_index({SESSION_USER_ID: str(uuid4())}, member=False)
    assert response.status_code == 403
    assert response.media_type == "text/html"
    assert INVITE_ONLY_HEADING in response.content
    assert SIGNED_IN_ADDRESS in response.content


def test_the_downloads_route_carries_the_membership_guard():
    downloads = next(
        handler for handler in _route_handlers() if "/downloads" in handler.paths
    )
    assert build_auth_guard in (downloads.guards or [])


def test_the_downloads_route_answers_the_shared_payload():
    with patch.object(email_message, "get_settings", email_settings):
        payload = asyncio.run(
            BuildController.downloads.fn(None, provide_public_base_url())
        )
    assert [platform["key"] for platform in payload["platforms"]] == [
        key for key, _ in PLATFORMS
    ]
    assert payload["install_command"].startswith("curl -fsSL ")


def test_the_downloads_route_is_handed_its_origin_by_the_shared_provider():
    """C3 gives /app/downloads no parameters. An origin the handler declares but the
    app does not provide would silently become a query parameter, so this asserts the
    registered route resolves it as a dependency — from the one shared provider."""
    app = Litestar(route_handlers=[BuildController], openapi_config=None)
    handler = next(iter(app.route_handler_method_map["/app/downloads"].values()))
    assert sorted(handler.resolve_dependencies()) == ["public_base_url"]
    assert (
        BuildController.dependencies["public_base_url"].dependency
        is provide_public_base_url
    )
