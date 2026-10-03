"""The invite routes over a real ASGI stack: what each link state answers, who may
redeem, and the admin JSON route that creates an invite and mails it. Drives the
handlers through HTTP rather than calling them, so the status codes, the redirects and
the dependency wiring are all under test."""

from __future__ import annotations

from collections.abc import Iterator
from datetime import timedelta
from uuid import UUID, uuid4

import pytest
from litestar.status_codes import (
    HTTP_200_OK,
    HTTP_401_UNAUTHORIZED,
    HTTP_201_CREATED,
    HTTP_302_FOUND,
    HTTP_400_BAD_REQUEST,
    HTTP_403_FORBIDDEN,
    HTTP_404_NOT_FOUND,
    HTTP_410_GONE,
)
from litestar.handlers import HTTPRouteHandler
from litestar.testing import TestClient
from skrift.auth.guards import auth_guard

from buildapp import email_message, invites
from buildapp.alpha_membership import holds_a_redeemed_invite
from buildapp.clock import utc_now
from buildapp.db_test_support import (
    asgi_app,
    in_memory_session_maker,
    session_backend_config,
    sign_in,
    stored_invites,
)
from buildapp.email_test_support import email_settings
from buildapp.invite_mail import INVITE_SUBJECT
from buildapp.invite_pages import OUTCOMES
from buildapp.invites import ALREADY_MEMBER, EMAIL_FIELD, EMAIL_MISMATCH, InviteState, invite_path
from buildapp.invites_controller import (
    INVITE_ROUTE_PATH,
    INVITES_API_PATH,
    InvitesController,
)
from buildapp.request_body import MALFORMED_JSON_MESSAGE

INVITED = "invitee@example.com"
OTHER_ADDRESS = "someone.else@example.com"


@pytest.fixture()
def client(monkeypatch, email_backend) -> Iterator[TestClient]:
    monkeypatch.setattr(email_message, "get_settings", email_settings)
    session_config = session_backend_config()
    app = asgi_app(
        [InvitesController],
        session_maker=in_memory_session_maker(),
        session_config=session_config,
    )
    app.state.email_backend = email_backend
    with TestClient(app=app, session_config=session_config) as test_client:
        yield test_client


def issue(client: TestClient, *, email: str = INVITED, kind="email_bound", **edits) -> str:
    """Put one invite in the database and hand back its raw token."""

    async def create() -> str:
        async with client.app.state.make_session() as session:
            if kind == "open_link":
                invite, raw = await invites.issue_open_invite(session, uuid4(), utc_now())
            else:
                invite, raw = await invites.issue_invite(session, email, uuid4(), utc_now())
            for field, value in edits.items():
                setattr(invite, field, value)
            await session.commit()
            return raw

    with client.portal() as portal:
        return portal.call(create)


def test_a_token_that_was_never_issued_is_not_found(client):
    response = client.get(invite_path("inv_never-issued"))
    assert response.status_code == HTTP_404_NOT_FOUND
    assert OUTCOMES[InviteState.UNKNOWN].heading in response.text


def test_public_invite_outcomes_and_redirects_do_not_cache_or_send_referrers(client):
    unknown = client.get(invite_path("inv_never-issued"), follow_redirects=False)
    open_token = issue(client)
    redirect = client.get(invite_path(open_token), follow_redirects=False)
    revoked_token = issue(client, revoked_at=utc_now())
    gone = client.get(invite_path(revoked_token), follow_redirects=False)
    expired_token = issue(client, expires_at=utc_now() - timedelta(seconds=1))
    expired = client.get(invite_path(expired_token), follow_redirects=False)
    used_token = issue(client, redeemed_by=uuid4(), redeemed_at=utc_now())
    used = client.get(invite_path(used_token), follow_redirects=False)
    sign_in(client, OTHER_ADDRESS)
    mismatch = client.get(invite_path(open_token), follow_redirects=False)
    matching_token = issue(client)
    sign_in(client, INVITED)
    redeemed = client.get(invite_path(matching_token), follow_redirects=False)
    for response in (unknown, redirect, gone, expired, used, mismatch, redeemed):
        assert response.headers["cache-control"] == "no-store"
        assert response.headers["referrer-policy"] == "no-referrer"


@pytest.mark.parametrize(
    ("slug", "reason"),
    [
        ("redeemed", InviteState.REDEEMED),
        ("revoked", InviteState.REVOKED),
        ("expired", InviteState.EXPIRED),
        ("wrong-account", EMAIL_MISMATCH),
        ("already-member", ALREADY_MEMBER),
    ],
)
def test_fixed_claim_outcome_url_renders_the_allowlisted_invite_page(client, slug, reason):
    response = client.get(f"/invite/outcome/{slug}", follow_redirects=False)
    assert response.status_code == OUTCOMES[reason].status_code
    assert OUTCOMES[reason].heading in response.text
    assert response.headers["cache-control"] == "no-store"
    assert response.headers["referrer-policy"] == "no-referrer"


def test_unknown_claim_outcome_slug_is_the_generic_invalid_page(client):
    response = client.get("/invite/outcome/<script>", follow_redirects=False)
    assert response.status_code == HTTP_404_NOT_FOUND
    assert OUTCOMES[InviteState.UNKNOWN].heading in response.text
    assert "<script>" not in response.text


def test_a_revoked_invite_is_gone(client):
    raw = issue(client, revoked_at=utc_now())
    response = client.get(invite_path(raw))
    assert response.status_code == HTTP_410_GONE
    assert OUTCOMES[InviteState.REVOKED].heading in response.text


def test_an_expired_invite_is_gone(client):
    raw = issue(client, expires_at=utc_now() - timedelta(seconds=1))
    response = client.get(invite_path(raw))
    assert response.status_code == HTTP_410_GONE
    assert OUTCOMES[InviteState.EXPIRED].heading in response.text


def test_an_already_redeemed_invite_points_at_the_app(client):
    raw = issue(client, redeemed_by=uuid4(), redeemed_at=utc_now())
    response = client.get(invite_path(raw))
    assert response.status_code == HTTP_200_OK
    assert OUTCOMES[InviteState.REDEEMED].heading in response.text
    assert 'href="/app/"' in response.text


def test_an_open_invite_sends_an_anonymous_visitor_to_login_and_back(client):
    raw = issue(client)
    response = client.get(invite_path(raw), follow_redirects=False)
    assert response.status_code == HTTP_302_FOUND
    assert response.headers["location"] == f"/auth/login?next={invite_path(raw)}"
    assert stored_invites(client)[0].redeemed_by is None


def test_an_open_invite_refuses_an_account_that_is_a_different_address(client):
    raw = issue(client)
    sign_in(client, OTHER_ADDRESS)
    response = client.get(invite_path(raw), follow_redirects=False)
    assert response.status_code == HTTP_403_FORBIDDEN
    assert OUTCOMES[EMAIL_MISMATCH].heading in response.text
    assert stored_invites(client)[0].redeemed_by is None


def test_redeeming_binds_the_invite_to_the_account_and_opens_the_app(client):
    raw = issue(client)
    user_id = sign_in(client, INVITED)
    response = client.get(invite_path(raw), follow_redirects=False)
    assert response.status_code == HTTP_302_FOUND
    assert response.headers["location"] == "/app/"
    invite = stored_invites(client)[0]
    assert invite.redeemed_by == user_id
    assert invite.redeemed_at is not None


def test_a_second_visit_to_a_redeemed_link_is_the_already_used_page(client):
    raw = issue(client)
    user_id = sign_in(client, INVITED)
    client.get(invite_path(raw), follow_redirects=False)
    again = client.get(invite_path(raw), follow_redirects=False)
    assert again.status_code == HTTP_200_OK
    assert OUTCOMES[InviteState.REDEEMED].heading in again.text
    assert stored_invites(client)[0].redeemed_by == user_id


@pytest.mark.parametrize("first_kind", ["email_bound", "open_link"])
@pytest.mark.parametrize("next_kind", ["email_bound", "open_link"])
def test_a_member_cannot_spend_another_link_and_escape_revocation(client, first_kind, next_kind):
    first_raw = issue(client, kind=first_kind)
    user_id = sign_in(client, INVITED)
    assert client.get(invite_path(first_raw), follow_redirects=False).status_code == 302
    extra_raw = issue(client, kind=next_kind)

    response = client.get(invite_path(extra_raw), follow_redirects=False)

    assert response.status_code == 200
    assert "You already have access to Build." in response.text
    assert 'href="/app/"' in response.text
    assert response.headers["cache-control"] == "no-store"
    extra, original = stored_invites(client)
    assert extra.redeemed_by is None
    assert extra.redeemed_at is None
    assert extra.email == ("" if next_kind == "open_link" else INVITED)

    async def revoke_original():
        async with client.app.state.make_session() as session:
            await invites.revoke_invite(session, original.id, utc_now())
            assert not await holds_a_redeemed_invite(session, user_id)

    with client.portal() as portal:
        portal.call(revoke_original)
    # A withdrawn membership does not prevent a later invitation to rejoin.
    assert client.get(invite_path(extra_raw), follow_redirects=False).status_code == 302
    assert stored_invites(client)[0].redeemed_by == user_id


ADMIN_ADDRESS = "operator@example.com"


def sign_in_as_admin(client: TestClient) -> UUID:
    return sign_in(client, ADMIN_ADDRESS, administrator=True)


def test_the_admin_route_creates_one_invite_and_mails_the_normalized_address(
    client, email_backend
):
    inviter = sign_in_as_admin(client)
    response = client.post(
        INVITES_API_PATH, json={EMAIL_FIELD: "  Invitee@Example.COM "}
    )
    assert response.status_code == HTTP_201_CREATED
    body = response.json()
    assert body["email"] == INVITED
    assert body["url"].startswith("https://getbuild.ing/invite/inv_")
    assert UUID(body["invite_id"])
    assert body["expires_at"].endswith("+00:00")
    assert [sent.to for sent in email_backend.sent] == [INVITED]
    assert email_backend.sent[0].subject == INVITE_SUBJECT
    assert body["url"] in email_backend.sent[0].text_body
    assert stored_invites(client)[0].invited_by == inviter


def test_admin_json_can_create_an_open_link_without_mailing_an_address(client, email_backend):
    inviter = sign_in_as_admin(client)
    response = client.post(INVITES_API_PATH, json={"kind": "open_link"})
    assert response.status_code == HTTP_201_CREATED
    body = response.json()
    assert body["kind"] == "open_link"
    assert body["email"] == ""
    assert body["url"].startswith("https://getbuild.ing/invite/inv_")
    (created,) = stored_invites(client)
    assert created.kind == "open_link"
    assert created.email == ""
    assert created.invited_by == inviter
    assert email_backend.sent == []


@pytest.mark.parametrize("payload", [
    {"kind": "unknown", "email": INVITED},
    {"kind": None, "email": INVITED},
    {"kind": "open_link", "email": INVITED},
    {"kind": "open_link", "email": ""},
])
def test_admin_json_refuses_invalid_kind_or_address_combination(client, email_backend, payload):
    sign_in_as_admin(client)
    response = client.post(INVITES_API_PATH, json=payload)
    assert response.status_code == HTTP_400_BAD_REQUEST
    assert stored_invites(client) == []
    assert email_backend.sent == []


def test_explicit_email_bound_kind_preserves_the_email_flow(client, email_backend):
    sign_in_as_admin(client)
    response = client.post(INVITES_API_PATH, json={"kind": "email_bound", "email": INVITED})
    assert response.status_code == HTTP_201_CREATED
    assert response.json()["kind"] == "email_bound"
    assert stored_invites(client)[0].kind == "email_bound"
    assert [sent.to for sent in email_backend.sent] == [INVITED]


def test_the_invite_the_admin_route_returns_is_the_one_a_visitor_can_redeem(client):
    sign_in_as_admin(client)
    url = client.post(INVITES_API_PATH, json={"email": INVITED}).json()["url"]
    raw = url.rsplit("/", 1)[-1]
    sign_in(client, INVITED)
    assert client.get(invite_path(raw), follow_redirects=False).status_code == (
        HTTP_302_FOUND
    )


def test_an_unparseable_address_is_refused_before_anything_is_stored(client, email_backend):
    sign_in_as_admin(client)
    response = client.post(INVITES_API_PATH, json={"email": "not-an-address"})
    assert response.status_code == HTTP_400_BAD_REQUEST
    assert response.json()["detail"] == invites.INVALID_ADDRESS_MESSAGE
    assert stored_invites(client) == []
    assert email_backend.sent == []


def test_a_body_that_is_not_an_object_is_refused(client):
    sign_in_as_admin(client)
    assert client.post(INVITES_API_PATH, json=["invitee@example.com"]).status_code == (
        HTTP_400_BAD_REQUEST
    )


def test_a_body_that_is_not_json_at_all_is_refused(client, email_backend):
    sign_in_as_admin(client)
    response = client.post(
        INVITES_API_PATH,
        content=b"{not json",
        headers={"content-type": "application/json"},
    )
    assert response.status_code == HTTP_400_BAD_REQUEST
    assert response.json()["detail"] == MALFORMED_JSON_MESSAGE
    assert stored_invites(client) == []
    assert email_backend.sent == []


def test_an_account_without_the_administrator_permission_may_not_invite(
    client, email_backend
):
    sign_in(client, OTHER_ADDRESS)
    response = client.post(INVITES_API_PATH, json={"email": INVITED})
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert stored_invites(client) == []
    assert email_backend.sent == []


def test_an_anonymous_caller_may_not_invite(client):
    response = client.post(INVITES_API_PATH, json={"email": INVITED})
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert stored_invites(client) == []


def test_the_admin_route_is_guarded_by_auth_and_the_administrator_permission():
    handler = next(
        handler
        for handler in vars(InvitesController).values()
        if isinstance(handler, HTTPRouteHandler) and INVITES_API_PATH in handler.paths
    )
    assert auth_guard in handler.guards
    assert any(
        getattr(guard, "permission", None) == "administrator" for guard in handler.guards
    )


def test_the_public_invite_link_carries_no_guard_at_all():
    handler = next(
        handler
        for handler in vars(InvitesController).values()
        if isinstance(handler, HTTPRouteHandler) and INVITE_ROUTE_PATH in handler.paths
    )
    assert not (handler.guards or [])
