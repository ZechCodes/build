from __future__ import annotations

from types import SimpleNamespace
from uuid import uuid4

from skrift.auth.session_keys import SESSION_USER_ID

import pytest
from litestar.exceptions import NotAuthorizedException, PermissionDeniedException

from buildapp.alpha_membership import INVITE_ONLY_DETAIL
from buildapp.desktop_auth import (
    DESKTOP_CLIENT_ID,
    build_auth_guard,
    desktop_token_user_id,
)


def test_desktop_access_token_identifies_its_user():
    user_id = uuid4()
    assert desktop_token_user_id(
        {
            "type": "access",
            "client_id": DESKTOP_CLIENT_ID,
            "scope": "openid profile email",
            "user_id": str(user_id),
        }
    ) == user_id


@pytest.mark.parametrize(
    "payload",
    [
        None,
        {},
        {"type": "refresh", "client_id": DESKTOP_CLIENT_ID, "scope": "openid"},
        {"type": "access", "client_id": "another-client", "scope": "openid"},
        {"type": "access", "client_id": DESKTOP_CLIENT_ID, "scope": "email"},
        {
            "type": "access",
            "client_id": DESKTOP_CLIENT_ID,
            "scope": "openid",
            "user_id": "not-a-uuid",
        },
    ],
)
def test_non_desktop_tokens_never_identify_a_user(payload):
    assert desktop_token_user_id(payload) is None


def _stub_membership(monkeypatch, *, member: bool) -> None:
    """Stand in for the membership check. Every caller states the answer it is
    testing against, so no test reads as the opposite of what it asserts."""

    async def require(db_session, user_id):
        if not member:
            raise PermissionDeniedException(INVITE_ONLY_DETAIL)

    monkeypatch.setattr("buildapp.desktop_auth.require_alpha_member", require)


@pytest.mark.asyncio
async def test_desktop_guard_rejects_a_missing_bearer():
    connection = SimpleNamespace(session={}, headers={}, scope={})
    with pytest.raises(NotAuthorizedException):
        await build_auth_guard(connection, None)


@pytest.mark.asyncio
async def test_desktop_guard_accepts_a_verified_desktop_token(monkeypatch):
    user_id = uuid4()
    _stub_membership(monkeypatch, member=True)

    async def verify_token(token, secret, db_session):
        assert token == "desktop-token"
        assert secret == "server-secret"
        assert db_session == "database"
        return {
            "type": "access",
            "client_id": DESKTOP_CLIENT_ID,
            "scope": "openid",
            "user_id": str(user_id),
        }

    class DatabaseContext:
        async def __aenter__(self):
            return "database"

        async def __aexit__(self, _error_type, _error, _traceback):
            return None

    monkeypatch.setattr("buildapp.desktop_auth.verify_oauth_token", verify_token)
    monkeypatch.setattr(
        "buildapp.desktop_auth.get_settings",
        lambda: SimpleNamespace(secret_key="server-secret"),
    )
    connection = SimpleNamespace(
        session={},
        headers={"authorization": "Bearer desktop-token"},
        scope={},
        app=SimpleNamespace(
            state=SimpleNamespace(session_maker_class=DatabaseContext)
        ),
    )

    await build_auth_guard(connection, None)

    assert connection.scope["state"]["build_user_id"] == str(user_id)


@pytest.mark.asyncio
async def test_a_session_that_is_not_an_alpha_member_is_refused(monkeypatch):
    _stub_membership(monkeypatch, member=False)
    connection = _session_connection(uuid4())
    with pytest.raises(PermissionDeniedException) as refusal:
        await build_auth_guard(connection, None)
    assert refusal.value.detail == INVITE_ONLY_DETAIL


@pytest.mark.asyncio
async def test_a_session_that_is_an_alpha_member_passes(monkeypatch):
    _stub_membership(monkeypatch, member=True)
    user_id = uuid4()
    connection = _session_connection(user_id)
    await build_auth_guard(connection, None)
    assert connection.scope["state"]["build_user_id"] == str(user_id)


@pytest.mark.asyncio
async def test_membership_is_checked_after_the_desktop_token_resolves_too(monkeypatch):
    _stub_membership(monkeypatch, member=False)
    monkeypatch.setattr(
        "buildapp.desktop_auth.verify_oauth_token",
        _verified_desktop_payload(uuid4()),
    )
    monkeypatch.setattr(
        "buildapp.desktop_auth.get_settings",
        lambda: SimpleNamespace(secret_key="server-secret"),
    )
    connection = SimpleNamespace(
        session={},
        headers={"authorization": "Bearer desktop-token"},
        scope={},
        app=SimpleNamespace(state=SimpleNamespace(session_maker_class=_DatabaseContext)),
    )
    with pytest.raises(PermissionDeniedException):
        await build_auth_guard(connection, None)


class _DatabaseContext:
    async def __aenter__(self):
        return "database"

    async def __aexit__(self, _error_type, _error, _traceback):
        return None


def _verified_desktop_payload(user_id):
    async def verify_token(token, secret, db_session):
        return {
            "type": "access",
            "client_id": DESKTOP_CLIENT_ID,
            "scope": "openid",
            "user_id": str(user_id),
        }

    return verify_token


def _session_connection(user_id) -> SimpleNamespace:
    return SimpleNamespace(
        session={SESSION_USER_ID: str(user_id)},
        headers={},
        scope={},
        app=SimpleNamespace(state=SimpleNamespace(session_maker_class=_DatabaseContext)),
    )
