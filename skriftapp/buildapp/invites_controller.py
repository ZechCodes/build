"""The two invite endpoints: the public link a guest opens, and the admin JSON route
that mints either invite kind and mails addressed invites.

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
from skrift.lib.email_backends import EmailBackend
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import invites, signup_invite
from buildapp.accounts import account_email
from buildapp.clock import utc_now
from buildapp.email_message import provide_email_backend, provide_public_base_url
from buildapp.invite_mail import invite_email_task
from buildapp.invite_kind import InviteKind
from buildapp.invite_pages import APP_PATH, OUTCOMES
from buildapp.invites import (
    EMAIL_FIELD,
    INVITE_PATH_PREFIX,
    InviteState,
    invite_path,
)
from buildapp.models import Invite
from buildapp.request_body import read_json_object
from buildapp.session_auth import login_redirect, session_user_id

INVITE_ROUTE_PATH = f"{INVITE_PATH_PREFIX}{{token:str}}"
INVITES_API_PATH = "/api/invites"
INVALID_INVITE_KIND_MESSAGE = "invalid invite kind"
OPEN_LINK_EMAIL_MESSAGE = "open links must not include an email address"
INVITE_RESPONSE_HEADERS = {
    "Cache-Control": "no-store",
    "Referrer-Policy": "no-referrer",
}


class InvitesController(Controller):
    path = ""
    dependencies = {
        "email_backend": Provide(provide_email_backend, sync_to_thread=False),
        "public_base_url": Provide(provide_public_base_url, sync_to_thread=False),
    }

    @get(INVITE_ROUTE_PATH, response_headers=INVITE_RESPONSE_HEADERS)
    async def open_invite(
        self, request: Request, token: str, db_session: AsyncSession
    ) -> Response | Redirect:
        """Public. Every state but OPEN is a page from the outcome table; an OPEN link
        needs an account before it can be spent, so a signed-out visitor carries the
        invite to the sign-in page, the one place an account can be made for it."""
        now = utc_now()
        invite = await invites.find_by_token(db_session, token)
        state = invites.invite_state(invite, now)
        if state is not InviteState.OPEN:
            return OUTCOMES[state].response()
        user_id = session_user_id(request)
        if user_id is None:
            signup_invite.carry(request, invite)
            return login_redirect(invite_path(token))
        return await self._redeem(db_session, invite, user_id, now)

    @staticmethod
    async def _redeem(
        db_session: AsyncSession, invite: Invite, user_id, now: datetime
    ) -> Response | Redirect:
        email = await account_email(db_session, user_id)
        redemption = await invites.claim_invite(db_session, invite, user_id, email, now)
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
        """Admin. Answers with the link; addressed invites also mail it in the
        background, fail-soft."""
        body = await read_json_object(request)
        try:
            kind = InviteKind(body.get("kind", InviteKind.EMAIL_BOUND))
        except (TypeError, ValueError) as bad_kind:
            raise ClientException(INVALID_INVITE_KIND_MESSAGE) from bad_kind
        if kind is InviteKind.OPEN_LINK:
            if EMAIL_FIELD in body:
                raise ClientException(OPEN_LINK_EMAIL_MESSAGE)
            invite, raw = await invites.issue_open_invite(
                db_session, session_user_id(request), utc_now()
            )
            background = None
        else:
            try:
                invite, raw = await invites.issue_invite(
                    db_session, str(body.get(EMAIL_FIELD, "")),
                    session_user_id(request), utc_now()
                )
            except ValueError as bad_address:
                raise ClientException(invites.INVALID_ADDRESS_MESSAGE) from bad_address
            background = invite_email_task(
                email_backend, invite.email,
                invites.invite_url(public_base_url, raw), public_base_url
            )
        url = invites.invite_url(public_base_url, raw)
        return Response(
            {
                "invite_id": str(invite.id),
                "kind": kind.value,
                "email": invite.email,
                "expires_at": invite.expires_at.isoformat(),
                "url": url,
            },
            background=background,
        )
