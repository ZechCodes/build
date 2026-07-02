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

from litestar.handlers import HTTPRouteHandler
from litestar.response import Redirect

from skrift.auth.session_keys import SESSION_USER_ID

from buildapp.controllers import BuildController


def _route_handlers() -> list[HTTPRouteHandler]:
    return [
        handler
        for handler in vars(BuildController).values()
        if isinstance(handler, HTTPRouteHandler)
    ]


def _call_index(session: dict):
    request = SimpleNamespace(session=session)
    return asyncio.run(BuildController.index.fn(None, request=request, db_session=None))


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


def test_no_async_handler_carries_sync_to_thread():
    for handler in _route_handlers():
        if asyncio.iscoroutinefunction(handler.fn):
            assert handler.sync_to_thread is None, (
                f"{list(handler.paths)}: sync_to_thread has no effect on an async "
                "callable and warns on every boot"
            )
