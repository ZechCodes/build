"""The two invite endpoints: the public link a guest opens, and the admin JSON route
that mints one and mails it.

Both parse, authorize and delegate — the rules are ``invites``', the copy and the status
codes are ``invite_pages``', and the mail is ``invite_mail``'s. The link handler never
branches on a state to pick a page: it looks the refusal up in ``OUTCOMES``.
"""

from __future__ import annotations

from datetime import datetime

from litestar import Controller, Request, get, post
from litestar.di import Provide
from litestar.exceptions import ClientException
from litestar.response import Redirect, Response
from litestar.status_codes import HTTP_201_CREATED
from skrift.auth.guards import Permission, auth_guard
from skrift.config import get_settings
from skrift.lib.email_backends import EmailBackend
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import invites
from buildapp.accounts import account_email
from buildapp.clock import utc_now
from buildapp.email_message import provide_email_backend, resolve_public_base_url
from buildapp.invite_mail import invite_email_task
from buildapp.invite_pages import APP_PATH, OUTCOMES
from buildapp.invites import INVITE_PATH_PREFIX, InviteState, invite_path
from buildapp.models import Invite
from buildapp.request_body import read_json_object
from buildapp.session_auth import session_user_id

INVITE_ROUTE_PATH = f"{INVITE_PATH_PREFIX}{{token:str}}"
INVITES_API_PATH = "/api/invites"
LOGIN_PATH_TEMPLATE = "/auth/login?next={next_path}"
EMAIL_FIELD = "email"



def provide_public_base_url() -> str:
    return resolve_public_base_url(get_settings())


def login_redirect_path(token: str) -> str:
    """Back here after login. Skrift stores ``next`` in its own session key and honours
    it after a sign-in AND after a passkey account creation, so a guest with no account
    yet makes one and lands on their invite."""
    return LOGIN_PATH_TEMPLATE.format(next_path=invite_path(token))


class InvitesController(Controller):
    path = ""
    dependencies = {
        "email_backend": Provide(provide_email_backend, sync_to_thread=False),
        "public_base_url": Provide(provide_public_base_url, sync_to_thread=False),
    }

    @get(INVITE_ROUTE_PATH)
    async def open_invite(
        self, request: Request, token: str, db_session: AsyncSession
    ) -> Response | Redirect:
        """Public. Every state but OPEN is a page from the outcome table; an OPEN link
        needs an account before it can be spent."""
        now = utc_now()
        invite = await invites.find_by_token(db_session, token)
        state = invites.invite_state(invite, now)
        if state is not InviteState.OPEN:
            return OUTCOMES[state].response()
        user_id = session_user_id(request)
        if user_id is None:
            return Redirect(login_redirect_path(token))
        return await self._redeem(db_session, invite, user_id, now)

    @staticmethod
    async def _redeem(
        db_session: AsyncSession, invite: Invite, user_id, now: datetime
    ) -> Response | Redirect:
        email = await account_email(db_session, user_id)
        redemption = invites.redeem(invite, user_id, email, now)
        if not redemption.ok:
            return OUTCOMES[redemption.reason].response()
        await db_session.commit()
        return Redirect(APP_PATH)

    @post(
        INVITES_API_PATH,
        guards=[auth_guard, Permission("administrator")],
        status_code=HTTP_201_CREATED,
    )
    async def create_invite(
        self,
        request: Request,
        db_session: AsyncSession,
        email_backend: EmailBackend,
        public_base_url: str,
    ) -> Response:
        """Admin. Answers with the link so the operator can pass it on by hand; the
        email goes out in the background, fail-soft."""
        body = await read_json_object(request)
        try:
            invite, raw = await invites.issue_invite(
                db_session, str(body.get(EMAIL_FIELD, "")), session_user_id(request), utc_now()
            )
        except ValueError as bad_address:
            raise ClientException(invites.INVALID_ADDRESS_MESSAGE) from bad_address
        url = invites.invite_url(public_base_url, raw)
        return Response(
            {
                "invite_id": str(invite.id),
                "email": invite.email,
                "expires_at": invite.expires_at.isoformat(),
                "url": url,
            },
            status_code=HTTP_201_CREATED,
            background=invite_email_task(email_backend, invite.email, url),
        )
