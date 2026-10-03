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
from buildapp.invite_pages import APP_PATH, INVITE_ONLY_HEADING, MISMATCH_HEADING, WAITLIST_PATH
from buildapp.invites import InviteState, invite_path
from buildapp.models import Invite
from buildapp.skrift_app_test_support import DATABASE_FILE, SECURE_ORIGIN, on_database

INVITED = "invitee@example.com"
OTHER_INVITED = "second@example.com"
STRANGER = "stranger@example.com"
LOGIN_PATH = "/auth/login"
SIGNIN_VIEW_PATH = "/auth/login?view=signin"
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


def issue_open(client: TestClient) -> str:
    async def work(session):
        _, raw = await invites.issue_open_invite(session, None, utc_now())
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

    def __init__(self, client: TestClient, path: str = LOGIN_PATH):
        self.client = client
        self.html = client.get(path).text
        self.csrf = CSRF_INPUT.search(self.html).group(1)

    def post(self, path: str, **fields):
        response = self.client.post(path, data={"_csrf": self.csrf, **fields})
        self.csrf = response.json().get("csrf_token") or self.csrf
        return response

    def create_account(self, email: str, *, opt_in: str | None = None):
        fields = {"email": email}
        if opt_in is not None:
            fields["product_email_opt_in"] = opt_in
        options = self.post(REGISTER_OPTIONS, **fields)
        if not options.is_success:
            return options
        return self.post(REGISTER_COMPLETE, credential=json.dumps({"id": "credential-1"}))


def signup_email_input(html: str) -> str | None:
    match = re.search(r'<input id="signup-email"[^>]*>', html)
    return match and match.group(0)


# --- the page ---------------------------------------------------------------------


def forms(html: str) -> list[str]:
    return re.findall(r'<form id="([^"]+)"', html)


def test_sign_in_asks_for_nothing(client):
    html = Page(client).html
    signin = re.search(r'<form id="signin-form".*?</form>', html, re.S).group(0)
    fields = re.findall(r"<input[^>]*>", signin)
    assert fields and all('type="hidden"' in field for field in fields), fields
    assert "Sign in with a passkey" in signin
    assert "webauthn" not in html


def test_without_an_invite_the_page_is_sign_in_alone(client):
    html = Page(client).html
    assert forms(html) == ["signin-form"]
    assert len(re.findall(r"<h[12][ >]", html)) == 1
    assert "Create account" not in html and "Create your" not in html
    aside = re.search(r'<p class="signin-hint signin-aside">(.*?)</p>', html, re.S).group(1)
    assert "invite-only" in aside and f'<a href="{WAITLIST_PATH}">Join the waitlist</a>' in aside
    assert "signin-button" not in aside


@pytest.mark.parametrize("view", ["signup", "signin", "SIGNIN", "x"])
def test_no_view_shows_an_account_form_without_an_invite(client, view):
    html = client.get(f"{LOGIN_PATH}?view={view}").text
    assert forms(html) == ["signin-form"]


def test_an_invite_visitor_sees_the_signup_form_alone_with_a_way_to_sign_in(client):
    open_invite(client, issue(client, INVITED))
    html = Page(client).html
    assert forms(html) == ["signup-form"]
    assert "<h1>Create your Build account</h1>" in html
    assert "Sign in with a passkey" not in html
    assert f'Already have an account? <a href="{SIGNIN_VIEW_PATH}">Sign in</a>' in html


def test_an_invite_visitor_can_switch_to_sign_in_and_back(client):
    open_invite(client, issue(client, INVITED))
    html = client.get(SIGNIN_VIEW_PATH).text
    assert forms(html) == ["signin-form"]
    assert "<h1>Sign in to Build</h1>" in html
    assert f'Have an invite for {INVITED}? <a href="{LOGIN_PATH}">Create your account</a>' in html
    assert forms(client.get(LOGIN_PATH).text) == ["signup-form"]


def test_the_passkey_login_route_shows_the_same_views(client):
    open_invite(client, issue(client, INVITED))
    assert forms(client.get("/auth/passkey/login").text) == ["signup-form"]
    assert forms(client.get("/auth/passkey/login?view=signin").text) == ["signin-form"]


@pytest.mark.parametrize("path", [LOGIN_PATH, "/auth/passkey/login"])
def test_auth_pages_do_not_cache_or_refer_an_invite_token(client, path):
    open_invite(client, issue(client, INVITED))
    response = client.get(path)
    assert response.headers["cache-control"] == "no-store"
    assert response.headers["referrer-policy"] == "no-referrer"


def test_an_unknown_view_is_the_default_and_never_reflected(client):
    open_invite(client, issue(client, INVITED))
    html = client.get(f"{LOGIN_PATH}?view=zz%3Cview%3Ezz").text
    assert forms(html) == ["signup-form"]
    assert "zz" not in html


def test_opening_an_invite_shows_its_address_prefilled_and_locked(client):
    raw = issue(client, INVITED)
    assert open_invite(client, raw).headers["location"].startswith(LOGIN_PATH)

    html = Page(client).html
    field = signup_email_input(html)
    assert field and f'value="{INVITED}"' in field and "readonly" in field
    assert 'name="name"' not in html


def test_open_link_offers_an_editable_address_and_unchecked_email_opt_in(client):
    open_invite(client, issue_open(client))
    html = Page(client).html
    field = signup_email_input(html)
    assert forms(html) == ["signup-form"]
    assert field and 'readonly' not in field and 'value=""' in field
    assert 'name="product_email_opt_in"' in html
    assert 'checked' not in re.search(r'<input[^>]+name="product_email_opt_in"[^>]*>', html).group(0)


def test_address_bound_invite_also_offers_unchecked_email_opt_in(client):
    open_invite(client, issue(client, INVITED))
    html = Page(client).html
    assert 'name="product_email_opt_in"' in html
    assert 'checked' not in re.search(r'<input[^>]+name="product_email_opt_in"[^>]*>', html).group(0)


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


def test_open_link_claims_the_address_the_new_account_uses(client, fake_authenticator):
    raw = issue_open(client)
    open_invite(client, raw)
    response = Page(client).create_account(INVITED)
    assert response.status_code == 201
    assert response.json()["redirect"] == APP_PATH

    async def work(session):
        return (await session.execute(select(Invite).where(Invite.redeemed_at.is_not(None)))).scalar_one()

    invite = on_database(client, work)
    assert invite.email == INVITED
    assert invite.redeemed_by is not None


def test_open_link_refuses_empty_address_before_passkey_prompt(client):
    open_invite(client, issue_open(client))
    response = Page(client).post(REGISTER_OPTIONS, email="  ")
    assert response.status_code == 403
    assert response.json()["error"] == INVITE_REQUIRED


@pytest.mark.parametrize("address", ["not-an-address", "qa@localhost", "a" * 245 + "@example.com"])
def test_open_link_refuses_addresses_the_waitlist_cannot_accept(client, address):
    open_invite(client, issue_open(client))
    response = Page(client).post(REGISTER_OPTIONS, email=address)
    assert response.status_code == 403
    assert response.json()["error"] == INVITE_REQUIRED


def test_open_link_options_cannot_be_completed_after_another_link_was_opened(client, fake_authenticator):
    open_invite(client, issue_open(client))
    page = Page(client)
    assert page.post(REGISTER_OPTIONS, email=INVITED).is_success
    open_invite(client, issue_open(client))
    response = page.post(REGISTER_COMPLETE, credential=json.dumps({"id": "credential-1"}))
    assert response.status_code == 403
    assert account_count(client) == 0


@pytest.mark.parametrize("open_link", [False, True], ids=["address-bound", "open-link"])
@pytest.mark.parametrize("opt_in", [None, "on"], ids=["no-consent", "consent"])
def test_signup_persists_explicit_email_consent_for_either_invite_kind(
    client, fake_authenticator, open_link, opt_in
):
    from buildapp.models import UserEmailPreference

    open_invite(client, issue_open(client) if open_link else issue(client, INVITED))
    assert Page(client).create_account(INVITED, opt_in=opt_in).status_code == 201

    async def work(session):
        return (await session.execute(select(UserEmailPreference))).scalar_one()

    preference = on_database(client, work)
    assert preference.product_email_opt_in is (opt_in == "on")
    assert (preference.product_email_opted_in_at is not None) is (opt_in == "on")


def test_signup_ignores_consent_forged_only_on_completion(client, fake_authenticator):
    from buildapp.models import UserEmailPreference

    open_invite(client, issue(client, INVITED))
    page = Page(client)
    assert page.post(REGISTER_OPTIONS, email=INVITED).is_success
    response = page.post(
        REGISTER_COMPLETE,
        credential=json.dumps({"id": "credential-1"}),
        product_email_opt_in="on",
    )
    assert response.status_code == 201

    async def work(session):
        return (await session.execute(select(UserEmailPreference))).scalar_one()

    preference = on_database(client, work)
    assert preference.product_email_opt_in is False
    assert preference.product_email_opted_in_at is None


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


def test_an_existing_account_signs_in_from_the_sign_in_view_and_meets_the_wrong_account_page(
    client, fake_authenticator, monkeypatch
):
    """Switching views keeps Skrift's ``next`` (the invite link, held in the session), so
    signing in to an account the invite is not for lands back on the link, which says so."""
    import skrift.controllers.auth as skrift_auth

    open_invite(client, issue(client, INVITED))
    assert Page(client).create_account(INVITED).status_code == 201
    client.cookies.clear()
    monkeypatch.setattr(
        skrift_auth,
        "complete_primary_passkey_authentication",
        lambda *args, **kwargs: SimpleNamespace(new_sign_count=1, verification_metadata={}),
    )

    raw = issue(client, OTHER_INVITED)
    client.get(open_invite(client, raw).headers["location"])
    page = Page(client, SIGNIN_VIEW_PATH)
    assert page.post("/auth/passkey/options").is_success
    signed_in = page.post("/auth/passkey/complete", credential=json.dumps({"id": "credential-1"}))

    assert signed_in.json()["redirect"] == invite_path(raw)
    landing = client.get(signed_in.json()["redirect"], follow_redirects=False)
    assert landing.status_code == 403
    assert MISMATCH_HEADING in landing.text


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
