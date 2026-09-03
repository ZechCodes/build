from __future__ import annotations

from types import SimpleNamespace
from uuid import uuid4

import pytest
from litestar.exceptions import NotAuthorizedException

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


@pytest.mark.asyncio
async def test_desktop_guard_rejects_a_missing_bearer():
    connection = SimpleNamespace(session={}, headers={}, scope={})
    with pytest.raises(NotAuthorizedException):
        await build_auth_guard(connection, None)


@pytest.mark.asyncio
async def test_desktop_guard_accepts_a_verified_desktop_token(monkeypatch):
    user_id = uuid4()

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
