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
from buildapp.models import Invite
from buildapp.waitlist_address import canonical_address

SIGNUP_INVITE_SESSION_KEY = "build_signup_invite"


def carry(request: Request, invite: Invite) -> None:
    """Bind this invite to the visitor's session; a later link replaces an earlier."""
    request.session[SIGNUP_INVITE_SESSION_KEY] = str(invite.id)


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
