"""The admin invites page: an admin-nav page behind the administrator permission, the
rows it builds (pure, so the template test needs no database), and the two CSRF-guarded
form posts that call the same service functions the JSON route does."""

from __future__ import annotations

from collections.abc import AsyncIterator, Iterator
from datetime import datetime, timedelta, timezone
from pathlib import Path
from uuid import UUID, uuid4

import pytest
from jinja2 import ChoiceLoader, DictLoader, Environment, FileSystemLoader
from litestar import Litestar
from litestar.di import Provide
from litestar.handlers import HTTPRouteHandler
from litestar.middleware.session.client_side import CookieBackendConfig
from litestar.testing import TestClient
from skrift.admin.navigation import ADMIN_NAV_TAG
from skrift.auth.guards import auth_guard
from skrift.auth.session_keys import SESSION_USER_ID
from skrift.forms.core import CSRF_FIELD_NAME, CSRF_SESSION_KEY
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from sqlalchemy.pool import StaticPool

from buildapp import invites, invites_admin
from buildapp.email_test_support import RecordingEmailBackend, email_settings
from buildapp.invites import INVITE_TTL, InviteState
from buildapp.invites_admin import (
    INVITES_ADMIN_PATH,
    INVITES_PAGE_ROUTE_PATH,
    REVOKE_PATH,
    SEND_INVITE_LABEL,
    InvitesAdminController,
    build_invites_dashboard,
)
from buildapp.models import Invite
from buildapp.test_invites_http import (
    IN_MEMORY_DATABASE_URL,
    SESSION_SECRET,
    add_account,
    create_skrift_tables,
)

NOW = datetime(2026, 9, 6, 12, 0, tzinfo=timezone.utc)
INVITER_ADDRESS = "operator@example.com"
INVITED = "invitee@example.com"
TEMPLATES_DIR = Path(__file__).resolve().parent.parent / "templates"
CSRF_TOKEN = "a-session-csrf-token"
CSRF_BODY = {CSRF_FIELD_NAME: CSRF_TOKEN}


def invite(**fields) -> Invite:
    defaults = dict(
        id=uuid4(),
        token_hash="hash",
        email=INVITED,
        invited_by=None,
        expires_at=NOW + INVITE_TTL,
        redeemed_by=None,
        redeemed_at=None,
        revoked_at=None,
        created_at=NOW,
    )
    defaults.update(fields)
    return Invite(**defaults)


def render_page(rows) -> str:
    """The real template, with admin/base.html stubbed and csrf_field() a fake — the
    same shape the transport admin template test uses."""
    environment = Environment(  # nosemgrep: python.flask.security.xss.audit.direct-use-of-jinja2.direct-use-of-jinja2
        loader=ChoiceLoader(
            [
                DictLoader(
                    {"admin/base.html": "{% block admin_content %}{% endblock %}"}
                ),
                FileSystemLoader(str(TEMPLATES_DIR)),
            ]
        ),
        autoescape=True,
    )
    return environment.get_template("admin/invites.html").render(
        invites=rows,
        site_name=lambda: "Build",
        csrf_field=lambda: f'<input type="hidden" name="{CSRF_FIELD_NAME}" value="t">',
    )


def test_the_page_is_an_admin_nav_page_behind_the_administrator_permission():
    page = next(
        handler
        for handler in vars(InvitesAdminController).values()
        if isinstance(handler, HTTPRouteHandler)
        and INVITES_PAGE_ROUTE_PATH in handler.paths
        and "GET" in handler.http_methods
    )
    assert ADMIN_NAV_TAG in (page.tags or [])
    assert auth_guard in (page.guards or [])
    assert any(
        getattr(guard, "permission", None) == "administrator" for guard in page.guards
    )
    assert page.opt["label"] == "Invites"


def test_every_route_on_the_page_requires_the_administrator_permission():
    handlers = [
        handler
        for handler in vars(InvitesAdminController).values()
        if isinstance(handler, HTTPRouteHandler)
    ]
    assert len(handlers) == 3  # the page, send, revoke
    for handler in handlers:
        assert any(
            getattr(guard, "permission", None) == "administrator"
            for guard in handler.guards
        ), handler.paths


def test_a_row_carries_the_state_word_and_the_address_that_sent_it():
    inviter = uuid4()
    rows = build_invites_dashboard(
        [invite(invited_by=inviter)], {inviter: INVITER_ADDRESS}, NOW
    )
    assert rows[0]["email"] == INVITED
    assert rows[0]["state"] == InviteState.OPEN.value
    assert rows[0]["invited_by"] == INVITER_ADDRESS


def test_a_row_whose_inviter_is_gone_says_so_rather_than_breaking():
    rows = build_invites_dashboard([invite(invited_by=uuid4())], {}, NOW)
    assert rows[0]["invited_by"] == "—"
    assert build_invites_dashboard([invite()], {}, NOW)[0]["invited_by"] == "—"


def test_a_redeemed_row_names_the_account_that_redeemed_it():
    redeemer = uuid4()
    rows = build_invites_dashboard(
        [invite(redeemed_by=redeemer, redeemed_at=NOW)], {redeemer: INVITED}, NOW
    )
    assert rows[0]["state"] == InviteState.REDEEMED.value
    assert rows[0]["redeemed_by"] == INVITED
    assert rows[0]["redeemed_at"] == NOW


def test_each_state_gets_its_word():
    rows = build_invites_dashboard(
        [
            invite(revoked_at=NOW),
            invite(expires_at=NOW - timedelta(days=1)),
            invite(),
        ],
        {},
        NOW,
    )
    assert [row["state"] for row in rows] == ["revoked", "expired", "open"]


def test_only_an_open_or_redeemed_invite_can_still_be_revoked():
    rows = build_invites_dashboard(
        [
            invite(),
            invite(redeemed_by=uuid4(), redeemed_at=NOW),
            invite(revoked_at=NOW),
            invite(expires_at=NOW - timedelta(days=1)),
        ],
        {},
        NOW,
    )
    assert [row["revocable"] for row in rows] == [True, True, False, False]


def test_the_template_carries_a_send_form_with_a_csrf_field():
    html = render_page(build_invites_dashboard([], {}, NOW))
    assert f'action="{INVITES_ADMIN_PATH}"' in html
    assert 'method="post"' in html
    assert CSRF_FIELD_NAME in html
    assert SEND_INVITE_LABEL in html


def test_the_template_offers_revoke_only_where_the_row_allows_it():
    open_invite = invite()
    revoked = invite(revoked_at=NOW)
    html = render_page(build_invites_dashboard([open_invite, revoked], {}, NOW))
    assert REVOKE_PATH.format(invite_id=open_invite.id) in html
    assert REVOKE_PATH.format(invite_id=revoked.id) not in html


def test_the_template_names_every_row_it_was_given():
    rows = build_invites_dashboard([invite(email="listed@example.com")], {}, NOW)
    assert "listed@example.com" in render_page(rows)


@pytest.fixture()
def email_backend() -> RecordingEmailBackend:
    return RecordingEmailBackend()


@pytest.fixture()
def admin_client(monkeypatch, email_backend) -> Iterator[TestClient]:
    """The two form POSTs over a real stack. They redirect rather than render, so the
    app needs no template engine — only the session the CSRF token lives in."""
    monkeypatch.setattr(invites_admin, "get_settings", email_settings)
    engine = create_async_engine(IN_MEMORY_DATABASE_URL, poolclass=StaticPool)
    make_session = async_sessionmaker(engine, expire_on_commit=False)

    async def provide_db_session() -> AsyncIterator[AsyncSession]:
        async with make_session() as session:
            yield session

    async def create_tables(app: Litestar) -> None:
        await create_skrift_tables(engine)

    async def dispose_engine(app: Litestar) -> None:
        await engine.dispose()

    session_config = CookieBackendConfig(secret=SESSION_SECRET)
    app = Litestar(
        route_handlers=[InvitesAdminController],
        dependencies={"db_session": Provide(provide_db_session)},
        middleware=[session_config.middleware],
        on_startup=[create_tables],
        on_shutdown=[dispose_engine],
    )
    app.state.email_backend = email_backend
    app.state.make_session = make_session
    app.state.session_maker_class = make_session
    with TestClient(app=app, session_config=session_config) as test_client:

        async def create_operator() -> UUID:
            async with make_session() as session:
                return await add_account(session, INVITER_ADDRESS, administrator=True)

        with test_client.portal() as portal:
            user_id = portal.call(create_operator)
        test_client.set_session_data(
            {SESSION_USER_ID: str(user_id), CSRF_SESSION_KEY: CSRF_TOKEN}
        )
        yield test_client


def stored(client: TestClient) -> list[Invite]:
    async def read() -> list[Invite]:
        async with client.app.state.make_session() as session:
            return await invites.all_invites(session)

    with client.portal() as portal:
        return portal.call(read)


def send_invite(client: TestClient, email: str, **body):
    return client.post(
        INVITES_ADMIN_PATH,
        data={"email": email, **body},
        follow_redirects=False,
    )


def test_the_send_form_issues_an_invite_and_mails_it(admin_client, email_backend):
    response = send_invite(admin_client, "  Invitee@Example.COM ", **CSRF_BODY)
    assert response.headers["location"] == INVITES_ADMIN_PATH
    assert [row.email for row in stored(admin_client)] == [INVITED]
    assert [sent.to for sent in email_backend.sent] == [INVITED]


def test_a_send_without_the_csrf_field_stores_and_sends_nothing(
    admin_client, email_backend
):
    send_invite(admin_client, INVITED)
    assert stored(admin_client) == []
    assert email_backend.sent == []


def test_an_address_the_form_cannot_send_to_stores_nothing(admin_client, email_backend):
    send_invite(admin_client, "not-an-address", **CSRF_BODY)
    assert stored(admin_client) == []
    assert email_backend.sent == []


def test_the_revoke_form_takes_the_seat_back(admin_client):
    send_invite(admin_client, INVITED, **CSRF_BODY)
    (issued,) = stored(admin_client)
    # A verified CSRF token is single-use: the page the operator lands on carries the
    # rotated one, so the revoke form submits that.
    rotated = admin_client.get_session_data()[CSRF_SESSION_KEY]
    admin_client.post(
        REVOKE_PATH.format(invite_id=issued.id),
        data={CSRF_FIELD_NAME: rotated},
        follow_redirects=False,
    )
    assert stored(admin_client)[0].revoked_at is not None


def test_a_revoke_without_the_csrf_field_changes_nothing(admin_client):
    send_invite(admin_client, INVITED, **CSRF_BODY)
    (issued,) = stored(admin_client)
    admin_client.post(
        REVOKE_PATH.format(invite_id=issued.id), data={}, follow_redirects=False
    )
    assert stored(admin_client)[0].revoked_at is None
