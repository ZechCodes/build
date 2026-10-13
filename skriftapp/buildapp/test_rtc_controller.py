"""The ICE-servers route: browser/desktop-authenticated by the shared guard, answering
the minted array under ``iceServers``, and turning a Cloudflare failure into a
502 rather than a silently degraded list.

Cloudflare is never called — ``ice_servers`` is replaced at the seam it exposes.
"""

from __future__ import annotations

import asyncio
from types import SimpleNamespace
from uuid import uuid4

import pytest
from litestar.exceptions import HTTPException, NotAuthorizedException
from litestar.handlers import HTTPRouteHandler
from litestar.status_codes import (
    HTTP_201_CREATED,
    HTTP_401_UNAUTHORIZED,
    HTTP_403_FORBIDDEN,
    HTTP_502_BAD_GATEWAY,
)
from litestar.testing import TestClient

from skrift.auth.session_keys import SESSION_USER_ID
from skrift.auth.tokens import create_signed_token

from buildapp import ice_servers as ice_servers_module
from buildapp.alpha_membership import INVITE_ONLY_DETAIL
from buildapp.db_test_support import (
    asgi_app,
    in_memory_session_maker,
    revoke_every_invite,
    session_backend_config,
    sign_in,
    sign_in_member,
    sign_out,
)
from buildapp.desktop_auth import DESKTOP_CLIENT_ID, build_auth_guard
from buildapp.ice_servers import (
    CF_TURN_KEY_API_TOKEN_ENV,
    CF_TURN_KEY_ID_ENV,
    STUN_ONLY,
    IceServersUnavailable,
)
from buildapp.rtc_controller import ICE_SERVERS_ROUTE_PATH, RtcController

KEY_ID = "key-abc"
API_TOKEN = "token-xyz"
OAUTH_SECRET = "test-server-secret-for-desktop-access-tokens"
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
    # The shape ``require_user`` reads: a session, and the request state the
    # desktop-token path checks after it (see test_session_auth.py).
    request = SimpleNamespace(session=session, scope={"state": {}})
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
    with pytest.raises(HTTPException) as raised:
        _mint(_signed_in())
    assert raised.value.status_code == HTTP_502_BAD_GATEWAY
    assert "403" in raised.value.detail


def test_an_anonymous_session_is_refused_before_any_key_is_read():
    with pytest.raises(NotAuthorizedException):
        _mint({})


def test_the_route_carries_the_shared_browser_and_desktop_guard():
    assert _route_handler().guards == [build_auth_guard]


def test_the_turn_key_never_reaches_the_browser(monkeypatch):
    _configure_key(monkeypatch)
    monkeypatch.setattr(
        ice_servers_module, "ice_servers", lambda key_id, api_token: MINTED_SERVERS
    )
    assert API_TOKEN not in str(_mint(_signed_in()).content)


@pytest.fixture()
def client(monkeypatch):
    monkeypatch.setattr(
        "buildapp.desktop_auth.get_settings",
        lambda: SimpleNamespace(secret_key=OAUTH_SECRET),
    )
    session_config = session_backend_config()
    app = asgi_app(
        [RtcController],
        session_maker=in_memory_session_maker(),
        session_config=session_config,
    )
    with TestClient(app=app, session_config=session_config) as test_client:
        yield test_client


def _desktop_headers(client, user_id):
    token = create_signed_token(
        {
            "type": "access",
            "client_id": DESKTOP_CLIENT_ID,
            "scope": "openid profile email",
            "user_id": str(user_id),
        },
        OAUTH_SECRET,
        900,
    )
    sign_out(client)
    return {"Authorization": f"Bearer {token}"}


@pytest.mark.parametrize("desktop", [False, True], ids=["browser", "desktop"])
def test_a_member_gets_minted_ice_servers(client, monkeypatch, desktop):
    user_id = sign_in_member(client)
    headers = _desktop_headers(client, user_id) if desktop else {}
    _configure_key(monkeypatch)
    monkeypatch.setattr(
        ice_servers_module, "ice_servers", lambda *_: MINTED_SERVERS
    )

    response = client.post(ICE_SERVERS_ROUTE_PATH, headers=headers)

    assert response.status_code == HTTP_201_CREATED
    assert response.json() == {"iceServers": MINTED_SERVERS}
    assert API_TOKEN not in response.text


@pytest.mark.parametrize("desktop", [False, True], ids=["browser", "desktop"])
@pytest.mark.parametrize("revoked", [False, True], ids=["uninvited", "revoked"])
def test_accounts_outside_the_alpha_are_refused_before_minting(
    client, monkeypatch, desktop, revoked
):
    if revoked:
        user_id = sign_in_member(client)
        revoke_every_invite(client)
    else:
        user_id = sign_in(client, "uninvited@example.com")
    headers = _desktop_headers(client, user_id) if desktop else {}

    mint_calls = []

    def unexpected_mint(*args):
        mint_calls.append(args)
        return []

    monkeypatch.setattr(ice_servers_module, "ice_servers", unexpected_mint)
    response = client.post(ICE_SERVERS_ROUTE_PATH, headers=headers)
    assert response.status_code == HTTP_403_FORBIDDEN
    assert response.json()["detail"] == INVITE_ONLY_DETAIL
    assert mint_calls == []


@pytest.mark.parametrize("authorization", [None, "Bearer invalid-desktop-token"])
def test_an_unauthenticated_request_is_rejected_by_the_guard(
    client, monkeypatch, authorization
):
    mint_calls = []

    def unexpected_mint(*args):
        mint_calls.append(args)
        return []

    monkeypatch.setattr(ice_servers_module, "ice_servers", unexpected_mint)
    headers = {"Authorization": authorization} if authorization else {}
    assert (
        client.post(ICE_SERVERS_ROUTE_PATH, headers=headers).status_code
        == HTTP_401_UNAUTHORIZED
    )
    assert mint_calls == []
