"""Who a download route will admit.

The install one-liner runs with no browser, so a download token in the query string is
a third way to say who you are — beside a session and the desktop bearer. It is only
ever an identity: membership is still asked afterwards, so a revoked invite kills a
live token."""

from __future__ import annotations

from collections.abc import Iterator
from datetime import timedelta
from uuid import UUID

import pytest
from litestar import Request, get
from litestar.status_codes import (
    HTTP_200_OK,
    HTTP_401_UNAUTHORIZED,
    HTTP_403_FORBIDDEN,
)
from litestar.testing import TestClient

from buildapp import download_tokens
from buildapp.alpha_membership import INVITE_ONLY_DETAIL
from buildapp.clock import utc_now
from buildapp.db_test_support import (
    asgi_app,
    in_memory_session_maker,
    revoke_every_invite,
    session_backend_config,
    sign_in,
    sign_in_member,
    sign_out,
)
from buildapp.desktop_auth import (
    BROWSER_IDENTIFIERS,
    DOWNLOAD_IDENTIFIERS,
    bearer_user,
    download_auth_guard,
    download_token_user,
    session_user,
)
from buildapp.session_auth import require_user

GUARDED_PATH = "/guarded"
MEMBER = "member@example.com"
WELL_FORMED_BUT_UNMINTED = "dl_" + "x" * 32


@get(GUARDED_PATH, guards=[download_auth_guard])
async def guarded(request: Request) -> dict:
    """Answers whoever the guard admitted, so a test can see the identity it set."""
    return {"user_id": str(require_user(request))}


@pytest.fixture()
def client() -> Iterator[TestClient]:
    session_config = session_backend_config()
    app = asgi_app(
        [guarded],
        session_maker=in_memory_session_maker(),
        session_config=session_config,
    )
    with TestClient(app=app, session_config=session_config) as test_client:
        yield test_client


def _mint(client: TestClient, user_id: UUID, *, minted_at_offset=timedelta()) -> str:
    async def create() -> str:
        async with client.app.state.make_session() as session:
            return await download_tokens.mint(
                session, user_id, utc_now() + minted_at_offset
            )

    with client.portal() as portal:
        return portal.call(create)


def test_the_browser_identifiers_are_the_session_and_the_desktop_bearer():
    assert BROWSER_IDENTIFIERS == (session_user, bearer_user)


def test_a_download_route_also_accepts_a_download_token():
    """One tuple, walked in order — a new way to identify a caller is an entry in it,
    never a branch in the guard."""
    assert DOWNLOAD_IDENTIFIERS == (*BROWSER_IDENTIFIERS, download_token_user)


def test_a_member_session_is_admitted(client):
    user_id = sign_in_member(client)
    response = client.get(GUARDED_PATH)
    assert response.status_code == HTTP_200_OK
    assert response.json() == {"user_id": str(user_id)}


def test_a_live_download_token_is_admitted_and_names_its_holder(client):
    user_id = sign_in_member(client)
    token = _mint(client, user_id)
    sign_out(client)

    response = client.get(GUARDED_PATH, params={"t": token})

    assert response.status_code == HTTP_200_OK
    assert response.json() == {"user_id": str(user_id)}


def test_nobody_at_all_is_refused(client):
    assert client.get(GUARDED_PATH).status_code == HTTP_401_UNAUTHORIZED


@pytest.mark.parametrize(
    "token",
    ["", "not-a-token", WELL_FORMED_BUT_UNMINTED, "dl_" + "x" * 31],
    ids=["empty", "malformed", "never minted", "wrong length"],
)
def test_a_token_that_names_nobody_is_refused(client, token):
    sign_in_member(client)
    sign_out(client)
    assert (
        client.get(GUARDED_PATH, params={"t": token}).status_code
        == HTTP_401_UNAUTHORIZED
    )


def test_an_expired_token_is_refused(client):
    user_id = sign_in_member(client)
    token = _mint(client, user_id, minted_at_offset=-download_tokens.TTL)
    sign_out(client)
    assert (
        client.get(GUARDED_PATH, params={"t": token}).status_code
        == HTTP_401_UNAUTHORIZED
    )


def test_a_spent_token_is_refused(client):
    user_id = sign_in_member(client)
    token = _mint(client, user_id)

    async def spend() -> None:
        async with client.app.state.make_session() as session:
            await download_tokens.spend(session, token, user_id)

    with client.portal() as portal:
        portal.call(spend)
    sign_out(client)

    assert (
        client.get(GUARDED_PATH, params={"t": token}).status_code
        == HTTP_401_UNAUTHORIZED
    )


def test_a_live_token_whose_holder_lost_the_alpha_is_refused_as_a_non_member(client):
    """A token is an identity, never an authorisation: revoking the invite closes the
    download in the ten minutes the token still has to live."""
    user_id = sign_in_member(client)
    token = _mint(client, user_id)
    revoke_every_invite(client)
    sign_out(client)

    response = client.get(GUARDED_PATH, params={"t": token})

    assert response.status_code == HTTP_403_FORBIDDEN
    assert response.json()["detail"] == INVITE_ONLY_DETAIL


def test_a_signed_in_non_member_is_refused_as_a_non_member(client):
    sign_in(client, "stranger@example.com")
    response = client.get(GUARDED_PATH)
    assert response.status_code == HTTP_403_FORBIDDEN
    assert response.json()["detail"] == INVITE_ONLY_DETAIL


def test_the_session_wins_over_a_token_someone_else_holds(client):
    """Identifiers are walked in order, so a browsing member is themselves and the
    token they did not present is not theirs to spend."""
    holder_id = sign_in_member(client, "holder@example.com")
    token = _mint(client, holder_id)
    browsing_id = sign_in_member(client, MEMBER)
    assert browsing_id != holder_id

    response = client.get(GUARDED_PATH, params={"t": token})

    assert response.json() == {"user_id": str(browsing_id)}
