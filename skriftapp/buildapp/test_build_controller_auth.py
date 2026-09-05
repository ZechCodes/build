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

from buildapp import controllers
from buildapp.controllers import BuildController
from buildapp.desktop_auth import build_auth_guard, download_auth_guard
from buildapp.email_message import provide_public_base_url
from buildapp.invite_pages import INVITE_ONLY_HEADING
from buildapp.release_assets import provide_asset_source

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


def test_the_download_routes_are_handed_everything_they_need_by_providers():
    """C3 gives these routes no query parameters. A value a handler declares but the
    app does not provide would silently become one, so this asserts the registered
    routes resolve their dependencies — from the two shared providers, so no handler
    reaches into the environment itself."""
    app = Litestar(route_handlers=[BuildController], openapi_config=None)
    for path in ("/app/downloads", "/app/downloads/token", "/app/downloads/{asset:str}"):
        handler = next(iter(app.route_handler_method_map[path].values()))
        assert sorted(handler.resolve_dependencies()) == [
            "asset_source",
            "public_base_url",
        ], path
    assert (
        BuildController.dependencies["public_base_url"].dependency
        is provide_public_base_url
    )
    assert (
        BuildController.dependencies["asset_source"].dependency is provide_asset_source
    )


def test_no_route_still_asks_for_a_releases_repository():
    """There is one repository now, named in ``releases``; a per-request repo value
    was the two-repo design's last handle."""
    assert "releases_repo" not in BuildController.dependencies
    for handler in _route_handlers():
        assert "releases_repo" not in inspect.signature(handler.fn).parameters


def test_the_asset_route_admits_an_install_lines_token_and_the_others_do_not():
    for path in ("/downloads", "/downloads/token"):
        handler = next(h for h in _route_handlers() if path in h.paths)
        assert handler.guards == [build_auth_guard], path
    asset_route = next(
        h for h in _route_handlers() if "/downloads/{asset:str}" in h.paths
    )
    assert asset_route.guards == [download_auth_guard]
