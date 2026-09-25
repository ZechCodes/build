"""The admin waitlist page: an admin-nav page beside Invites behind the administrator
permission, one row per signup newest first with the state and button its newest invite
calls for, a literal search, and one CSRF-guarded send that invites or resends through
the invite domain and mails the new link."""

from __future__ import annotations

import re
from collections.abc import Iterator
from datetime import datetime, timedelta, timezone
from uuid import UUID, uuid4

import pytest
from litestar.handlers import HTTPRouteHandler
from litestar.status_codes import HTTP_401_UNAUTHORIZED
from litestar.testing import TestClient
from skrift.admin.navigation import ADMIN_NAV_TAG
from skrift.auth.guards import auth_guard
from skrift.auth.session_keys import SESSION_USER_ID
from skrift.forms.core import CSRF_FIELD_NAME, CSRF_SESSION_KEY
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import email_message, invites
from buildapp.db_test_support import (
    admin_template_environment,
    asgi_app,
    in_memory_session_maker,
    session_backend_config,
    sign_in,
    stored_invites,
)
from buildapp.email_test_support import email_settings
from buildapp.invite_mail import GETTING_STARTED, INVITE_SUBJECT
from buildapp.invites import INVITE_PATH_PREFIX, INVITE_TTL, InviteState
from buildapp.invites_admin import InvitesAdminController
from buildapp.models import Invite, WaitlistSignup
from buildapp.waitlist_admin import (
    ADMIN_PREFIX,
    INVITE_LABEL,
    RESEND_LABEL,
    SEARCH_FIELD,
    SEND_PATH,
    TEMPLATE_NAME,
    WAITLIST_ADMIN_PATH,
    WAITLIST_PAGE_ROUTE_PATH,
    WaitlistAdminController,
    build_waitlist_dashboard,
    waitlist_page_context,
    waitlist_page_url,
    waitlist_signups,
)

NOW = datetime(2026, 9, 6, 12, 0, tzinfo=timezone.utc)
OPERATOR = "operator@example.com"
CSRF_TOKEN = "a-session-csrf-token"
INVITE_LINK = re.compile(r"https://getbuild\.ing/invite/(inv_[A-Za-z0-9_-]+)")


def signup(email: str, age: timedelta = timedelta(days=1)) -> WaitlistSignup:
    return WaitlistSignup(id=uuid4(), email=email, created_at=NOW - age)


def invite(email: str, **fields) -> Invite:
    defaults = dict(
        id=uuid4(),
        token_hash="hash",
        email=email,
        invited_by=None,
        expires_at=NOW + INVITE_TTL,
        redeemed_by=None,
        redeemed_at=None,
        revoked_at=None,
        created_at=NOW - timedelta(hours=2),
    )
    defaults.update(fields)
    return Invite(**defaults)


def render_page(rows, search: str = "") -> str:
    return admin_template_environment().get_template(TEMPLATE_NAME).render(
        **waitlist_page_context(rows, search),
        site_name=lambda: "Build",
        csrf_field=lambda: f'<input type="hidden" name="{CSRF_FIELD_NAME}" value="t">',
    )


# --- the page ---------------------------------------------------------------------


def handlers() -> list[HTTPRouteHandler]:
    return [
        handler
        for handler in vars(WaitlistAdminController).values()
        if isinstance(handler, HTTPRouteHandler)
    ]


def test_the_page_is_an_admin_nav_page_beside_invites():
    (page,) = [h for h in handlers() if "GET" in h.http_methods]
    assert WAITLIST_PAGE_ROUTE_PATH in page.paths
    assert ADMIN_NAV_TAG in (page.tags or [])
    assert auth_guard in page.guards
    assert page.opt["label"] == "Waitlist"
    invites_page = next(
        h
        for h in vars(InvitesAdminController).values()
        if isinstance(h, HTTPRouteHandler) and "GET" in h.http_methods
    )
    # The nav sorts by (order, label): same order, so Invites then Waitlist, adjacent.
    assert page.opt["order"] == invites_page.opt["order"]


def test_every_route_requires_the_administrator_permission():
    assert len(handlers()) == 2  # the page, send
    for handler in handlers():
        assert any(
            getattr(guard, "permission", None) == "administrator"
            for guard in handler.guards
        ), handler.paths


def test_each_state_gets_its_phrase_and_button():
    signups = [
        signup("none@example.com"),
        signup("invited@example.com"),
        signup("joined@example.com"),
        signup("expired@example.com"),
        signup("revoked@example.com"),
    ]
    newest = {
        "invited@example.com": invite("invited@example.com"),
        "joined@example.com": invite(
            "joined@example.com", redeemed_by=uuid4(), redeemed_at=NOW - timedelta(hours=1)
        ),
        "expired@example.com": invite(
            "expired@example.com", expires_at=NOW - timedelta(days=1)
        ),
        "revoked@example.com": invite("revoked@example.com", revoked_at=NOW),
    }
    rows = build_waitlist_dashboard(signups, newest, NOW)
    assert [
        (row["status"].label, row["action"] and row["action"]["label"]) for row in rows
    ] == [
        ("Not invited", INVITE_LABEL),
        ("Invited", RESEND_LABEL),
        ("Joined", None),
        ("Expired", RESEND_LABEL),
        ("Revoked", RESEND_LABEL),
    ]
    assert rows[0]["action"]["confirm"] == "Send an invite to none@example.com?"
    assert rows[1]["action"]["confirm"] == (
        "Resend the invite to invited@example.com? The previous link stops working."
    )
    assert rows[1]["status"].moment.relative == "2 hours ago"
    assert rows[2]["status"].moment.relative == "1 hour ago"


def test_the_template_renders_state_moment_button_and_confirmation():
    none, invited, joined = (
        signup("none@example.com", timedelta(days=3)),
        signup("invited@example.com"),
        signup("joined@example.com"),
    )
    newest = {
        "invited@example.com": invite("invited@example.com"),
        "joined@example.com": invite(
            "joined@example.com", redeemed_by=uuid4(), redeemed_at=NOW
        ),
    }
    html = render_page(build_waitlist_dashboard([none, invited, joined], newest, NOW))
    assert "3 days ago" in html
    assert 'title="2026-09-03 12:00 UTC"' in html
    assert "Invited <time" in html and "Joined <time" in html
    # Invite and Resend each open a confirmation; the joined row has no button.
    assert html.count("popovertarget=") == 4  # an opener and a Cancel per action row
    assert f'popovertarget="confirm-{joined.id}"' not in html
    assert "Send an invite to none@example.com?" in html
    assert "The previous link stops working." in html
    assert f'action="{SEND_PATH.format(signup_id=none.id)}"' in html
    assert f'action="{SEND_PATH.format(signup_id=joined.id)}"' not in html
    assert html.count(CSRF_FIELD_NAME) == 2


def test_the_template_carries_the_search_box_and_keeps_the_term_in_each_send():
    rows = build_waitlist_dashboard([signup("a@example.com")], {}, NOW)
    html = render_page(rows, search="exam")
    assert f'action="{WAITLIST_ADMIN_PATH}"' in html
    assert f'name="{SEARCH_FIELD}" value="exam"' in html
    assert html.count(f'name="{SEARCH_FIELD}"') == 2  # the box, and the send's hidden field


def test_an_empty_search_says_nothing_matched():
    assert "No signups match" in render_page([], search="zzz")
    assert "No one is on the waitlist yet." in render_page([])


def test_the_page_url_keeps_the_search():
    assert waitlist_page_url("") == WAITLIST_ADMIN_PATH
    assert waitlist_page_url("a b&c") == f"{WAITLIST_ADMIN_PATH}?q=a+b%26c"


# --- the query ----------------------------------------------------------------------


async def seed_signups(db: AsyncSession, *emails_oldest_first: str) -> None:
    for age, email in enumerate(reversed(emails_oldest_first), start=1):
        db.add(signup(email, timedelta(hours=age)))
    await db.commit()


@pytest.mark.asyncio
async def test_signups_list_newest_first(db):
    await seed_signups(db, "first@example.com", "second@example.com", "third@example.com")
    assert [row.email for row in await waitlist_signups(db, "")] == [
        "third@example.com",
        "second@example.com",
        "first@example.com",
    ]


@pytest.mark.asyncio
async def test_search_filters_by_substring_case_blind_and_literally(db):
    await seed_signups(db, "alice@example.com", "bob@example.org", "al_ex@example.com")
    assert [row.email for row in await waitlist_signups(db, "EXAMPLE.COM")] == [
        "al_ex@example.com",
        "alice@example.com",
    ]
    assert [row.email for row in await waitlist_signups(db, "l_e")] == [
        "al_ex@example.com"
    ]
    assert await waitlist_signups(db, "%") == []


# --- sending over HTTP --------------------------------------------------------------

SIGNED_UP = "Waitlisted@example.com"
SIGNED_UP_NORMALIZED = "waitlisted@example.com"


@pytest.fixture()
def client(monkeypatch, email_backend) -> Iterator[TestClient]:
    monkeypatch.setattr(email_message, "get_settings", email_settings)
    session_config = session_backend_config()

    async def seed(session: AsyncSession) -> None:
        session.add(WaitlistSignup(email=SIGNED_UP_NORMALIZED))
        await session.commit()

    app = asgi_app(
        [WaitlistAdminController, InvitesAdminController],
        session_maker=in_memory_session_maker(),
        session_config=session_config,
        seed=seed,
    )
    app.state.email_backend = email_backend
    with TestClient(app=app, session_config=session_config) as test_client:
        yield test_client


@pytest.fixture()
def admin_client(client) -> TestClient:
    user_id = sign_in(client, OPERATOR, administrator=True)
    client.set_session_data({SESSION_USER_ID: str(user_id), CSRF_SESSION_KEY: CSRF_TOKEN})
    return client


def signup_id(client: TestClient) -> UUID:
    async def read() -> UUID:
        async with client.app.state.make_session() as session:
            (row,) = await waitlist_signups(session, "")
            return row.id

    with client.portal() as portal:
        return portal.call(read)


def open_link(client: TestClient, raw: str) -> InviteState:
    """What the link in an email is now, read through the domain the /invite route uses."""

    async def read() -> InviteState:
        async with client.app.state.make_session() as session:
            found = await invites.find_by_token(session, raw)
            return invites.invite_state(found, datetime.now(timezone.utc))

    with client.portal() as portal:
        return portal.call(read)


def press_send(client: TestClient, *, csrf: bool = True, search: str = ""):
    token = client.get_session_data().get(CSRF_SESSION_KEY) if csrf else None
    data = {SEARCH_FIELD: search}
    if token:
        data[CSRF_FIELD_NAME] = token
    return client.post(
        SEND_PATH.format(signup_id=signup_id(client)), data=data, follow_redirects=False
    )


def link_in(sent) -> str:
    (raw,) = set(INVITE_LINK.findall(sent.text_body))
    assert f"{INVITE_PATH_PREFIX}{raw}" in sent.html_body
    return raw


def test_invite_issues_one_invite_and_mails_the_link_and_the_steps(
    admin_client, email_backend
):
    response = press_send(admin_client)
    assert response.status_code in (301, 302, 303)
    assert response.headers["location"] == WAITLIST_ADMIN_PATH
    (issued,) = stored_invites(admin_client)
    assert issued.email == SIGNED_UP_NORMALIZED
    assert invites.invite_state(issued, datetime.now(timezone.utc)) is InviteState.OPEN
    (sent,) = email_backend.sent
    assert sent.to == SIGNED_UP_NORMALIZED
    assert sent.subject == INVITE_SUBJECT
    assert open_link(admin_client, link_in(sent)) is InviteState.OPEN
    for step in GETTING_STARTED.steps:
        assert step.text in sent.text_body


def test_resend_revokes_the_live_link_and_mails_a_new_one(admin_client, email_backend):
    press_send(admin_client)
    first = link_in(email_backend.sent[0])
    press_send(admin_client)
    second = link_in(email_backend.sent[1])
    assert first != second
    assert open_link(admin_client, first) is InviteState.REVOKED
    assert open_link(admin_client, second) is InviteState.OPEN
    live = [
        row
        for row in stored_invites(admin_client)
        if invites.invite_state(row, datetime.now(timezone.utc)) is InviteState.OPEN
    ]
    assert len(live) == 1


def test_a_send_keeps_the_search_the_operator_was_looking_at(admin_client):
    response = press_send(admin_client, search="waitl")
    assert response.headers["location"] == f"{WAITLIST_ADMIN_PATH}?q=waitl"


def test_a_joined_signup_is_not_sent_another_invite(admin_client, email_backend):
    press_send(admin_client)

    async def redeem() -> None:
        async with admin_client.app.state.make_session() as session:
            (row,) = await invites.all_invites(session)
            row.redeemed_by, row.redeemed_at = uuid4(), datetime.now(timezone.utc)
            await session.commit()

    with admin_client.portal() as portal:
        portal.call(redeem)
    press_send(admin_client)
    assert len(stored_invites(admin_client)) == 1
    assert len(email_backend.sent) == 1


def test_a_send_without_the_csrf_field_stores_and_sends_nothing(
    admin_client, email_backend
):
    press_send(admin_client, csrf=False)
    assert stored_invites(admin_client) == []
    assert email_backend.sent == []


def test_a_signup_that_is_gone_sends_nothing(admin_client, email_backend):
    admin_client.post(
        SEND_PATH.format(signup_id=uuid4()),
        data={CSRF_FIELD_NAME: CSRF_TOKEN},
        follow_redirects=False,
    )
    assert stored_invites(admin_client) == []
    assert email_backend.sent == []


def test_a_signed_in_non_administrator_is_refused_the_page_and_the_send(
    client, email_backend
):
    """Skrift's Permission guard answers a missing permission with 401, as it does on
    every admin route and the JSON invite route (``test_invites_http``)."""
    user_id = sign_in(client, "someone@example.com")
    client.set_session_data({SESSION_USER_ID: str(user_id), CSRF_SESSION_KEY: CSRF_TOKEN})
    assert client.get(WAITLIST_ADMIN_PATH).status_code == HTTP_401_UNAUTHORIZED
    response = client.post(
        SEND_PATH.format(signup_id=signup_id(client)),
        data={CSRF_FIELD_NAME: CSRF_TOKEN},
        follow_redirects=False,
    )
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert stored_invites(client) == []
    assert email_backend.sent == []


def test_the_prefix_and_suffix_are_each_written_once():
    assert WaitlistAdminController.path == ADMIN_PREFIX
    assert WAITLIST_ADMIN_PATH == f"{ADMIN_PREFIX}{WAITLIST_PAGE_ROUTE_PATH}"
    assert SEND_PATH.startswith(f"{WAITLIST_ADMIN_PATH}/")
