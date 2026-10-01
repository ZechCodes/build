"""Account creation is invite-only (#314), through the whole app as Skrift builds it.

Opening an invite link signed out binds that invite to the visitor's session; the
sign-in page then offers account creation for the invite's address alone, and Skrift's
passkey registration routes refuse any other address whatever the form posts. The new
account redeems the invite in the same request, so it lands in /app/ as a member."""

from __future__ import annotations

import json
import re
import sqlite3
from datetime import timedelta
from types import SimpleNamespace

import pytest
from litestar.testing import TestClient
from skrift.db.models.user import User
from sqlalchemy import func, select

from buildapp import invites
from buildapp.clock import utc_now
from buildapp.db_test_support import add_account
from buildapp.invite_pages import APP_PATH, INVITE_ONLY_HEADING, WAITLIST_PATH
from buildapp.invites import InviteState, invite_path
from buildapp.models import Invite
from buildapp.skrift_app_test_support import DATABASE_FILE, SECURE_ORIGIN, on_database

INVITED = "invitee@example.com"
OTHER_INVITED = "second@example.com"
STRANGER = "stranger@example.com"
LOGIN_PATH = "/auth/login"
REGISTER_OPTIONS = "/auth/passkey/register/options"
REGISTER_COMPLETE = "/auth/passkey/register/complete"
INVITE_REQUIRED = "invite_required"
CSRF_INPUT = re.compile(r'name="_csrf" value="([^"]+)"')


@pytest.fixture()
def client(skrift_app):
    with TestClient(skrift_app, base_url=SECURE_ORIGIN) as client:
        yield client


@pytest.fixture()
def fake_authenticator(monkeypatch):
    """Stand in for the browser's half of registration: any credential posted to
    register/complete verifies, as a real authenticator's would."""
    import skrift.controllers.auth as skrift_auth

    def verified(request, settings, *, method_key, credential):
        return SimpleNamespace(
            credential_id=credential["id"],
            public_key=b"public-key",
            sign_count=0,
            transports=["internal"],
            enrollment_metadata={},
        )

    monkeypatch.setattr(skrift_auth, "complete_primary_passkey_registration", verified)


def issue(client: TestClient, email: str, *, expires_in: timedelta | None = None) -> str:
    async def work(session):
        invite, raw = await invites.issue_invite(session, email, None, utc_now())
        if expires_in is not None:
            invite.expires_at = utc_now() + expires_in
            await session.commit()
        return raw

    return on_database(client, work)


def stored_invite(client: TestClient, email: str) -> Invite:
    async def work(session):
        return (await session.execute(select(Invite).where(Invite.email == email))).scalar_one()

    return on_database(client, work)


def account_count(client: TestClient) -> int:
    async def work(session):
        return (await session.execute(select(func.count()).select_from(User))).scalar_one()

    return on_database(client, work)


def revoke(client: TestClient, email: str) -> None:
    async def work(session):
        invite = (await session.execute(select(Invite).where(Invite.email == email))).scalar_one()
        await invites.revoke_invite(session, invite.id, utc_now())

    on_database(client, work)


def open_invite(client: TestClient, raw: str):
    return client.get(invite_path(raw), follow_redirects=False)


class Page:
    """The sign-in page as the browser holds it: its markup and the CSRF token its
    forms carry, which every passkey POST rotates."""

    def __init__(self, client: TestClient):
        self.client = client
        self.html = client.get(LOGIN_PATH).text
        self.csrf = CSRF_INPUT.search(self.html).group(1)

    def post(self, path: str, **fields):
        response = self.client.post(path, data={"_csrf": self.csrf, **fields})
        self.csrf = response.json().get("csrf_token") or self.csrf
        return response

    def create_account(self, email: str):
        options = self.post(REGISTER_OPTIONS, email=email)
        if not options.is_success:
            return options
        return self.post(REGISTER_COMPLETE, credential=json.dumps({"id": "credential-1"}))


def signup_email_input(html: str) -> str | None:
    match = re.search(r'<input id="signup-email"[^>]*>', html)
    return match and match.group(0)


# --- the page ---------------------------------------------------------------------


def test_sign_in_asks_for_nothing(client):
    html = Page(client).html
    signin = re.search(r'<form id="signin-form".*?</form>', html, re.S).group(0)
    fields = re.findall(r"<input[^>]*>", signin)
    assert fields and all('type="hidden"' in field for field in fields), fields
    assert "Sign in with a passkey" in signin
    assert "webauthn" not in html


def test_without_an_invite_there_is_no_create_account_form(client):
    html = Page(client).html
    assert 'id="signup-form"' not in html
    assert "invite-only" in html
    assert f'href="{WAITLIST_PATH}"' in html


def test_opening_an_invite_shows_its_address_prefilled_and_locked(client):
    raw = issue(client, INVITED)
    assert open_invite(client, raw).headers["location"].startswith(LOGIN_PATH)

    html = Page(client).html
    field = signup_email_input(html)
    assert field and f'value="{INVITED}"' in field and "readonly" in field
    assert 'name="name"' not in html


@pytest.mark.parametrize(
    "spoil",
    [
        lambda client, raw: revoke(client, INVITED),
        lambda client, raw: on_database(client, lambda s: _expire(s, INVITED)),
    ],
    ids=["revoked", "expired"],
)
def test_an_invite_spoiled_after_opening_no_longer_offers_an_account(client, spoil):
    raw = issue(client, INVITED)
    open_invite(client, raw)
    spoil(client, raw)
    assert 'id="signup-form"' not in Page(client).html


async def _expire(session, email):
    invite = (await session.execute(select(Invite).where(Invite.email == email))).scalar_one()
    invite.expires_at = utc_now() - timedelta(seconds=1)
    await session.commit()


def test_a_link_already_past_its_time_binds_nothing(client):
    raw = issue(client, INVITED, expires_in=timedelta(seconds=-1))
    assert open_invite(client, raw).status_code == 410
    assert 'id="signup-form"' not in Page(client).html


# --- the server refuses whatever the page posts ------------------------------------


def test_registration_options_without_an_invite_are_refused(client):
    response = Page(client).post(REGISTER_OPTIONS, email=STRANGER)
    assert response.status_code == 403
    assert response.json()["error"] == INVITE_REQUIRED
    assert response.json()["csrf_token"]


def test_registration_options_for_another_address_are_refused(client):
    open_invite(client, issue(client, INVITED))
    response = Page(client).post(REGISTER_OPTIONS, email=STRANGER)
    assert response.status_code == 403
    assert response.json()["error"] == INVITE_REQUIRED


def test_registration_options_for_the_invited_address_are_given(client):
    open_invite(client, issue(client, INVITED))
    response = Page(client).post(REGISTER_OPTIONS, email=f"  {INVITED.upper()} ")
    assert response.is_success
    assert response.json()["options"]["challenge"]


def test_an_invited_address_that_already_has_an_account_gets_skrifts_generic_refusal(client):
    on_database(client, lambda session: add_account(session, INVITED))
    open_invite(client, issue(client, INVITED))
    response = Page(client).post(REGISTER_OPTIONS, email=INVITED)
    assert response.status_code == 400
    assert response.json()["error"] == "invalid_request"


def test_completing_after_the_session_moved_to_another_invite_is_refused(client, fake_authenticator):
    open_invite(client, issue(client, INVITED))
    page = Page(client)
    assert page.post(REGISTER_OPTIONS, email=INVITED).is_success
    open_invite(client, issue(client, OTHER_INVITED))

    response = page.post(REGISTER_COMPLETE, credential=json.dumps({"id": "credential-1"}))

    assert response.status_code == 403
    assert response.json()["error"] == INVITE_REQUIRED
    assert account_count(client) == 0


def test_completing_after_the_invite_was_revoked_is_refused(client, fake_authenticator):
    open_invite(client, issue(client, INVITED))
    page = Page(client)
    assert page.post(REGISTER_OPTIONS, email=INVITED).is_success
    revoke(client, INVITED)

    response = page.post(REGISTER_COMPLETE, credential=json.dumps({"id": "credential-1"}))

    assert response.status_code == 403
    assert account_count(client) == 0


# --- one flow from link to member ---------------------------------------------------


def test_creating_the_account_redeems_the_invite_and_opens_build(client, fake_authenticator):
    open_invite(client, issue(client, INVITED))

    response = Page(client).create_account(INVITED)

    assert response.status_code == 201
    assert response.json()["redirect"] == APP_PATH
    invite = stored_invite(client, INVITED)
    assert invites.invite_state(invite, utc_now()) is InviteState.REDEEMED
    assert invite.redeemed_by is not None


def test_the_new_account_has_no_name_to_think_about(client, fake_authenticator):
    open_invite(client, issue(client, INVITED))
    Page(client).create_account(INVITED)

    async def work(session):
        return (await session.execute(select(User).where(User.email == INVITED))).scalar_one()

    assert on_database(client, work).name is None


def test_signing_in_drops_the_bound_invite(client, fake_authenticator):
    """Skrift rotates the session at sign-in, so an invite opened by one visitor never
    rides along into whatever the next account on this browser does."""
    open_invite(client, issue(client, INVITED))
    assert Page(client).create_account(INVITED).status_code == 201
    assert 'id="signup-form"' not in Page(client).html


# --- Skrift's own checks come first ----------------------------------------------


def registration_without_an_invite(client: TestClient) -> Page:
    """A registration Skrift has under way whose invite is gone, so Build's check would
    refuse anything that reached it."""
    open_invite(client, issue(client, INVITED))
    page = Page(client)
    assert page.post(REGISTER_OPTIONS, email=INVITED).is_success
    revoke(client, INVITED)
    return page


@pytest.mark.parametrize("path", [REGISTER_OPTIONS, REGISTER_COMPLETE])
def test_a_post_without_a_csrf_token_gets_skrifts_refusal(client, path):
    registration_without_an_invite(client)
    response = client.post(path, data={"email": INVITED, "credential": "{}"})
    assert response.status_code == 400
    assert response.json()["error"] == "invalid_csrf"


@pytest.mark.parametrize("path", [REGISTER_OPTIONS, REGISTER_COMPLETE])
def test_a_post_with_the_wrong_csrf_token_gets_skrifts_refusal(client, path):
    registration_without_an_invite(client)
    response = client.post(path, data={"_csrf": "not-the-token", "email": INVITED, "credential": "{}"})
    assert response.status_code == 400
    assert response.json()["error"] == "invalid_csrf"


@pytest.mark.parametrize("path", [REGISTER_OPTIONS, REGISTER_COMPLETE])
def test_a_post_skrift_refuses_reads_no_invite(client, monkeypatch, path):
    import buildapp.auth_controller as auth_controller

    registration_without_an_invite(client)

    async def unread(*args):
        raise AssertionError("looked up the carried invite for a request Skrift refuses")

    monkeypatch.setattr(auth_controller, "carried_invite", unread)
    response = client.post(path, data={"email": INVITED, "credential": "{}"})
    assert response.json()["error"] == "invalid_csrf"


def test_the_token_returned_with_a_refusal_is_the_one_still_in_force(client):
    page = Page(client)
    issued = page.csrf
    refused = page.post(REGISTER_OPTIONS, email=STRANGER)
    assert refused.json()["error"] == INVITE_REQUIRED
    assert refused.json()["csrf_token"] == issued

    open_invite(client, issue(client, INVITED))
    assert page.post(REGISTER_OPTIONS, email=INVITED).is_success


@pytest.mark.parametrize("step", ["register/options", "register/complete"])
def test_an_unknown_provider_is_skrifts_404(client, step):
    page = registration_without_an_invite(client)
    response = client.post(f"/auth/nope/{step}", data={"_csrf": page.csrf, "email": INVITED, "credential": "{}"})
    assert response.status_code == 404


# --- an account the invite could not be spent on is not a member --------------------


def test_an_invite_revoked_mid_registration_leaves_an_account_outside_the_app(client, monkeypatch):
    """Creating the account and redeeming the invite are separate commits. Revoke the
    invite between them (inside the authenticator's step, after Build's check) and the
    account exists unredeemed: /app/ must still refuse it."""
    import skrift.controllers.auth as skrift_auth

    def revoke_then_verify(request, settings, *, method_key, credential):
        with sqlite3.connect(DATABASE_FILE) as connection:
            connection.execute("UPDATE invites SET revoked_at = CURRENT_TIMESTAMP")
        return SimpleNamespace(
            credential_id=credential["id"],
            public_key=b"public-key",
            sign_count=0,
            transports=["internal"],
            enrollment_metadata={},
        )

    monkeypatch.setattr(skrift_auth, "complete_primary_passkey_registration", revoke_then_verify)
    open_invite(client, issue(client, INVITED))

    response = Page(client).create_account(INVITED)

    assert response.status_code == 201
    assert response.json()["redirect"] != APP_PATH
    assert stored_invite(client, INVITED).redeemed_at is None
    app = client.get(APP_PATH, follow_redirects=False)
    assert app.status_code == 403
    assert INVITE_ONLY_HEADING in app.text


# --- no other route under the passkey-only config makes an account ------------------


def test_the_passkey_callback_creates_no_account(client):
    open_invite(client, issue(client, INVITED))
    client.get("/auth/passkey/callback?code=anything&state=anything", follow_redirects=False)
    assert account_count(client) == 0


def test_the_dummy_login_creates_no_account(client):
    open_invite(client, issue(client, INVITED))
    page = Page(client)
    response = client.post(
        "/auth/dummy-login", data={"_csrf": page.csrf, "email": INVITED, "name": "x"}, follow_redirects=False
    )
    assert response.status_code == 404
    assert account_count(client) == 0
