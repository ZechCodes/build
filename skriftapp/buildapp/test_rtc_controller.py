"""The ICE-servers route: session-authenticated by the reusable guard, answering
the minted array under ``iceServers``, and turning a Cloudflare failure into a
502 rather than a silently degraded list.

Cloudflare is never called — ``ice_servers`` is replaced at the seam it exposes.
"""

from __future__ import annotations

import asyncio
import os
from types import SimpleNamespace
from uuid import uuid4

import pytest
from litestar import Litestar
from litestar.exceptions import NotAuthorizedException
from litestar.handlers import HTTPRouteHandler
from litestar.middleware.session.client_side import CookieBackendConfig
from litestar.status_codes import HTTP_401_UNAUTHORIZED, HTTP_502_BAD_GATEWAY
from litestar.testing import TestClient

from skrift.auth.guards import auth_guard
from skrift.auth.session_keys import SESSION_USER_ID

from buildapp import ice_servers as ice_servers_module
from buildapp.ice_servers import (
    CF_TURN_KEY_API_TOKEN_ENV,
    CF_TURN_KEY_ID_ENV,
    STUN_ONLY,
    IceServersUnavailable,
)
from buildapp.rtc_controller import ICE_SERVERS_ROUTE_PATH, RtcController

KEY_ID = "key-abc"
API_TOKEN = "token-xyz"
MINTED_SERVERS = [
    {
        "urls": ["turn:turn.cloudflare.com:3478?transport=udp"],
        "username": "minted-username",
        "credential": "minted-credential",
    }
]


@pytest.fixture(autouse=True)
def no_turn_key(monkeypatch):
    monkeypatch.delenv(CF_TURN_KEY_ID_ENV, raising=False)
    monkeypatch.delenv(CF_TURN_KEY_API_TOKEN_ENV, raising=False)


def _configure_key(monkeypatch) -> None:
    monkeypatch.setenv(CF_TURN_KEY_ID_ENV, KEY_ID)
    monkeypatch.setenv(CF_TURN_KEY_API_TOKEN_ENV, API_TOKEN)


def _mint(session: dict):
    request = SimpleNamespace(session=session)
    return asyncio.run(RtcController.mint_ice_servers.fn(None, request=request))


def _signed_in() -> dict:
    return {SESSION_USER_ID: str(uuid4())}


def _route_handler() -> HTTPRouteHandler:
    return next(
        handler
        for handler in vars(RtcController).values()
        if isinstance(handler, HTTPRouteHandler)
        and ICE_SERVERS_ROUTE_PATH in handler.paths
    )


def test_the_minted_array_is_answered_under_ice_servers(monkeypatch):
    _configure_key(monkeypatch)
    asked = []

    def fake_ice_servers(key_id: str, api_token: str) -> list[dict]:
        asked.append((key_id, api_token))
        return MINTED_SERVERS

    monkeypatch.setattr(ice_servers_module, "ice_servers", fake_ice_servers)
    assert _mint(_signed_in()).content == {"iceServers": MINTED_SERVERS}
    assert asked == [(KEY_ID, API_TOKEN)]


def test_without_a_configured_key_the_answer_is_stun_only():
    assert _mint(_signed_in()).content == {"iceServers": STUN_ONLY}


def test_a_cloudflare_failure_is_a_502_not_a_degraded_list(monkeypatch):
    _configure_key(monkeypatch)

    def failing_ice_servers(key_id: str, api_token: str) -> list[dict]:
        raise IceServersUnavailable("cloudflare answered 403 for TURN credentials")

    monkeypatch.setattr(ice_servers_module, "ice_servers", failing_ice_servers)
    with pytest.raises(Exception) as raised:
        _mint(_signed_in())
    assert raised.value.status_code == HTTP_502_BAD_GATEWAY
    assert "403" in raised.value.detail


def test_an_anonymous_session_is_refused_before_any_key_is_read():
    with pytest.raises(NotAuthorizedException):
        _mint({})


def test_the_route_carries_the_reusable_auth_guard():
    assert auth_guard in (_route_handler().guards or [])


def test_the_turn_key_never_reaches_the_browser(monkeypatch):
    _configure_key(monkeypatch)
    monkeypatch.setattr(
        ice_servers_module, "ice_servers", lambda key_id, api_token: MINTED_SERVERS
    )
    assert API_TOKEN not in str(_mint(_signed_in()).content)


def test_an_unauthenticated_request_is_rejected_by_the_guard():
    app = Litestar(
        route_handlers=[RtcController],
        middleware=[CookieBackendConfig(secret=os.urandom(16)).middleware],
    )
    with TestClient(app=app) as client:
        assert client.post(ICE_SERVERS_ROUTE_PATH).status_code == HTTP_401_UNAUTHORIZED
