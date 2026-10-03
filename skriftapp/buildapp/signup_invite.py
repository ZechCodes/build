"""The invite a signed-out visitor carries from their invite link to account creation.

Opening an open invite link signed out binds that invite to the visitor's session — its
id, never the token or the address, and never anything the visitor can name in a URL or
a form. Account creation then reads the address from that binding alone. Skrift's
session is a signed cookie, so the binding cannot be forged or edited, and Skrift
clears it when it rotates the session at sign-in."""

from __future__ import annotations

from datetime import datetime
from uuid import UUID

from litestar import Request
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.invites import InviteState, invite_state
from buildapp.invite_kind import InviteKind
from buildapp.models import Invite
from buildapp.waitlist_address import canonical_address, normalize_waitlist_address

SIGNUP_INVITE_SESSION_KEY = "build_signup_invite"
SIGNUP_OPTIONS_SESSION_KEY = "build_signup_options"


def carry(request: Request, invite: Invite) -> None:
    """Bind this invite to the visitor's session; a later link replaces an earlier."""
    request.session[SIGNUP_INVITE_SESSION_KEY] = str(invite.id)
    request.session.pop(SIGNUP_OPTIONS_SESSION_KEY, None)


def _carried_id(request: Request) -> UUID | None:
    try:
        return UUID(str(request.session.get(SIGNUP_INVITE_SESSION_KEY)))
    except ValueError:
        return None


async def carried_invite(
    request: Request, db_session: AsyncSession, now: datetime
) -> Invite | None:
    """The invite this session carries, while it is still open; otherwise ``None`` —
    revoked, expired, redeemed or never bound all mean no account can be made."""
    invite_id = _carried_id(request)
    if invite_id is None:
        return None
    invite = await db_session.get(Invite, invite_id)
    return invite if invite_state(invite, now) is InviteState.OPEN else None


def admits(invite: Invite | None, email: str) -> bool:
    """Whether an account for ``email`` may be created on this invite."""
    return invite is not None and canonical_address(email) == canonical_address(invite.email)


def admitted_address(invite: Invite | None, email: str) -> str | None:
    """The identity Skrift may register. An open link binds at claim; an addressed
    invite keeps its original address check. Both reject an empty identity."""
    address = canonical_address(email)
    if invite is None or not address:
        return None
    if invite.kind == InviteKind.OPEN_LINK.value:
        return normalize_waitlist_address(email)
    return address if admits(invite, email) else None


def remember_options(request: Request, invite: Invite, email: str, opt_in: bool) -> None:
    """Keep the exact link and email behind Skrift's passkey options in the signed
    session. The completion request may not choose a different link or consent."""
    request.session[SIGNUP_OPTIONS_SESSION_KEY] = {
        "invite_id": str(invite.id),
        "email": canonical_address(email),
        "product_email_opt_in": opt_in,
    }


def options_for(request: Request, invite: Invite | None, email: str) -> dict | None:
    options = request.session.get(SIGNUP_OPTIONS_SESSION_KEY)
    if not isinstance(options, dict) or invite is None:
        return None
    if options.get("invite_id") != str(invite.id):
        return None
    if options.get("email") != admitted_address(invite, email):
        return None
    return options
