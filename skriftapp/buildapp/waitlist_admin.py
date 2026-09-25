"""The admin waitlist page: everyone who asked for an invite, newest first, what became
of the invite each was sent, and one button per row that sends it.

The button reads Invite for an address never invited and Resend for one whose link is
out, expired or revoked; a signup that has joined gets none. Either opens a confirmation
first (a popover, so the page needs no script under the CSP), and Send posts to one
route that revokes whatever link the address still holds and issues a fresh one through
``invites.reissue_invite`` — the same domain the invites page and the JSON route use.
"""

from __future__ import annotations

from datetime import datetime
from typing import Any, Iterable, Mapping
from urllib.parse import urlencode
from uuid import UUID

from litestar import Controller, Request, get, post
from litestar.di import Provide
from litestar.response import Redirect
from litestar.response import Template as TemplateResponse
from skrift.admin.helpers import get_admin_context
from skrift.admin.navigation import ADMIN_NAV_TAG
from skrift.auth.guards import Permission, auth_guard
from skrift.flash import flash_error, flash_success, get_flash_messages
from skrift.forms.core import verify_csrf
from skrift.lib.email_backends import EmailBackend
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import invites
from buildapp.clock import utc_now
from buildapp.email_message import provide_email_backend, provide_public_base_url
from buildapp.invite_mail import send_invite_email
from buildapp.invite_status import InviteStatus, invite_status, moment
from buildapp.invites import InviteState
from buildapp.invites_admin import (
    ADMIN_PREFIX,
    CSRF_REFUSED_MESSAGE,
    INVITE_REFUSED_MESSAGE,
    sent_message,
)
from buildapp.models import Invite, WaitlistSignup
from buildapp.session_auth import session_user_id

WAITLIST_PAGE_ROUTE_PATH = "/waitlist"
SEND_SUFFIX = "/invite"
SEND_ROUTE_PATH = f"{WAITLIST_PAGE_ROUTE_PATH}/{{signup_id:uuid}}{SEND_SUFFIX}"
WAITLIST_ADMIN_PATH = f"{ADMIN_PREFIX}{WAITLIST_PAGE_ROUTE_PATH}"
SEND_PATH = f"{WAITLIST_ADMIN_PATH}/{{signup_id}}{SEND_SUFFIX}"
TEMPLATE_NAME = "admin/waitlist.html"
#: The search box's query parameter, and the hidden field a send carries it back in so
#: the operator lands on the same filtered list.
SEARCH_FIELD = "q"

INVITE_LABEL = "Invite"
RESEND_LABEL = "Resend"
SEND_LABEL = "Send"
CANCEL_LABEL = "Cancel"
INVITE_CONFIRM = "Send an invite to {email}?"
RESEND_CONFIRM = "Resend the invite to {email}? The previous link stops working."

#: The states whose row offers a resend. A joined signup has nothing to send.
RESENDABLE_STATES = frozenset(
    {InviteState.OPEN, InviteState.EXPIRED, InviteState.REVOKED}
)

ALREADY_JOINED_MESSAGE = "{email} has already joined."
SIGNUP_MISSING_MESSAGE = "That signup is no longer on the waitlist."


def row_action(status: InviteStatus, email: str) -> dict[str, str] | None:
    """The button a row carries and the question its confirmation asks, or None."""
    if status.state is InviteState.UNKNOWN:
        return {"label": INVITE_LABEL, "confirm": INVITE_CONFIRM.format(email=email)}
    if status.state in RESENDABLE_STATES:
        return {"label": RESEND_LABEL, "confirm": RESEND_CONFIRM.format(email=email)}
    return None


def build_waitlist_dashboard(
    signups: Iterable[WaitlistSignup], newest: Mapping[str, Invite], now: datetime
) -> list[dict[str, Any]]:
    """Everything the template shows, pure over its inputs — so the template test needs
    no database and no clock."""
    rows = []
    for signup in signups:
        status = invite_status(newest.get(signup.email), now)
        rows.append(
            {
                "signup_id": str(signup.id),
                "email": signup.email,
                "signed_up": moment(signup.created_at, now),
                "status": status,
                "action": row_action(status, signup.email),
                "send_path": SEND_PATH.format(signup_id=signup.id),
            }
        )
    return rows


def waitlist_page_context(rows: list[dict[str, Any]], search: str) -> dict[str, Any]:
    return {
        "signups": rows,
        "search": search,
        "search_field": SEARCH_FIELD,
        "page_path": WAITLIST_ADMIN_PATH,
        "send_label": SEND_LABEL,
        "cancel_label": CANCEL_LABEL,
    }


async def waitlist_signups(
    db_session: AsyncSession, search: str
) -> list[WaitlistSignup]:
    """Every signup, newest first, narrowed to addresses containing ``search``. The
    term is matched literally: a ``%`` or ``_`` in it is not a wildcard."""
    query = select(WaitlistSignup).order_by(WaitlistSignup.created_at.desc())
    if search:
        query = query.where(
            WaitlistSignup.email.contains(search.lower(), autoescape=True)
        )
    return list((await db_session.execute(query)).scalars())


class WaitlistAdminController(Controller):
    """Look down the list, invite whoever is next."""

    path = ADMIN_PREFIX
    guards = [auth_guard]
    dependencies = {
        "email_backend": Provide(provide_email_backend, sync_to_thread=False),
        "public_base_url": Provide(provide_public_base_url, sync_to_thread=False),
    }

    @get(
        WAITLIST_PAGE_ROUTE_PATH,
        tags=[ADMIN_NAV_TAG],
        guards=[auth_guard, Permission("administrator")],
        opt={"label": "Waitlist", "icon": "list", "order": 94},
    )
    async def waitlist_page(
        self, request: Request, db_session: AsyncSession
    ) -> TemplateResponse:
        ctx = await get_admin_context(request, db_session)
        search = request.query_params.get(SEARCH_FIELD, "").strip()
        signups = await waitlist_signups(db_session, search)
        newest = await invites.newest_invites_by_email(
            db_session, (signup.email for signup in signups)
        )
        return TemplateResponse(
            TEMPLATE_NAME,
            context={
                "flash_messages": get_flash_messages(request),
                **waitlist_page_context(
                    build_waitlist_dashboard(signups, newest, utc_now()), search
                ),
                **ctx,
            },
        )

    @post(SEND_ROUTE_PATH, guards=[auth_guard, Permission("administrator")])
    async def send_invite(
        self,
        request: Request,
        signup_id: UUID,
        db_session: AsyncSession,
        email_backend: EmailBackend,
        public_base_url: str,
    ) -> Redirect:
        """Invite and Resend are one action: revoke any link still out, issue a fresh
        one, mail it. Inline rather than a background task, as on the invites page, so
        the flash follows a send that happened."""
        form = await request.form()
        back = str(form.get(SEARCH_FIELD, "")).strip()
        if not await verify_csrf(request):
            return _flashed(request, CSRF_REFUSED_MESSAGE, back, ok=False)
        signup = await db_session.get(WaitlistSignup, signup_id)
        if signup is None:
            return _flashed(request, SIGNUP_MISSING_MESSAGE, back, ok=False)
        newest = await invites.newest_invites_by_email(db_session, (signup.email,))
        if invites.invite_state(newest.get(signup.email), utc_now()) is (
            InviteState.REDEEMED
        ):
            message = ALREADY_JOINED_MESSAGE.format(email=signup.email)
            return _flashed(request, message, back, ok=False)
        try:
            invite, raw = await invites.reissue_invite(
                db_session, signup.email, session_user_id(request), utc_now()
            )
        except ValueError:
            return _flashed(request, INVITE_REFUSED_MESSAGE, back, ok=False)
        mailed = await send_invite_email(
            email_backend, invite.email, invites.invite_url(public_base_url, raw)
        )
        return _flashed(request, sent_message(invite.email, mailed), back, ok=mailed)


def waitlist_page_url(search: str) -> str:
    """The page, with the search the operator was looking at when they sent."""
    if not search:
        return WAITLIST_ADMIN_PATH
    return f"{WAITLIST_ADMIN_PATH}?{urlencode({SEARCH_FIELD: search})}"


def _flashed(request: Request, message: str, search: str, *, ok: bool) -> Redirect:
    (flash_success if ok else flash_error)(request, message)
    return Redirect(waitlist_page_url(search))
