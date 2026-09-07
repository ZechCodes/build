"""The invite domain: minting, the five states an invite link can be in, the redemption
rule, and the two service functions the JSON route and the admin form both call.

Pure of the web stack on purpose — nothing here imports Litestar. A handler asks for a
state or a redemption and gets a value back; what that value means to a visitor (status
code and copy) is ``invite_pages``, and what it means for access is ``alpha_membership``.
"""

from __future__ import annotations

import secrets
from dataclasses import dataclass
from datetime import datetime, timedelta
from enum import Enum
from uuid import UUID

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.models import Invite
from buildapp.token_hash import token_hash
from buildapp.waitlist_address import canonical_address, normalize_waitlist_address

#: How long an invite link stays open. One redemption, then it is spent.
INVITE_TTL = timedelta(days=14)
INVITE_PATH_PREFIX = "/invite/"
TOKEN_PREFIX = "inv_"
TOKEN_BYTES = 32
INVALID_ADDRESS_MESSAGE = "invalid email address"
#: The one name the address goes by on the way in: the JSON key of the admin route's
#: body and the name of the admin page's form field are the same fact.
EMAIL_FIELD = "email"


class InviteState(Enum):
    """What an invite link is, in the order the states take precedence. A link that is
    both revoked and expired is REVOKED — the operator's decision outranks the clock."""

    UNKNOWN = "unknown"
    REVOKED = "revoked"
    REDEEMED = "redeemed"
    EXPIRED = "expired"
    OPEN = "open"


class EmailMismatch:
    """The one refusal that is not a state: the link is open, but this account is not
    the address it was sent to. A sentinel rather than a state so ``invite_state`` stays
    a function of the row alone."""

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return "EMAIL_MISMATCH"


EMAIL_MISMATCH = EmailMismatch()

RedemptionRefusal = InviteState | EmailMismatch


@dataclass(frozen=True)
class Redemption:
    ok: bool
    reason: RedemptionRefusal


def invite_path(token: str) -> str:
    return f"{INVITE_PATH_PREFIX}{token}"


def invite_url(public_base_url: str, token: str) -> str:
    return f"{public_base_url.rstrip('/')}{invite_path(token)}"


def mint_invite_token() -> str:
    """A fresh raw token. Shown once, in the email; the row keeps only its hash."""
    return f"{TOKEN_PREFIX}{secrets.token_urlsafe(TOKEN_BYTES)}"


def invite_state(invite: Invite | None, now: datetime) -> InviteState:
    if invite is None:
        return InviteState.UNKNOWN
    if invite.revoked_at is not None:
        return InviteState.REVOKED
    if invite.redeemed_at is not None:
        return InviteState.REDEEMED
    if invite.expires_at <= now:
        return InviteState.EXPIRED
    return InviteState.OPEN


def redeem(
    invite: Invite, user_id: UUID, user_email: str, now: datetime
) -> Redemption:
    """Bind an open invite to the account that opened it, or say why not. Mutates the
    row only on success, so a refusal leaves the link exactly as it was."""
    state = invite_state(invite, now)
    if state is not InviteState.OPEN:
        return Redemption(ok=False, reason=state)
    # Identity, not deliverability: the stored address is already normalized, and a
    # seeded invite for an address the waitlist would refuse (the dev stack's
    # qa@localhost) must still be redeemable by that account.
    if canonical_address(user_email) != canonical_address(invite.email):
        return Redemption(ok=False, reason=EMAIL_MISMATCH)
    invite.redeemed_by = user_id
    invite.redeemed_at = now
    return Redemption(ok=True, reason=state)


async def issue_invite(
    db_session: AsyncSession, email: str, invited_by: UUID | None, now: datetime
) -> tuple[Invite, str]:
    """Create and commit one invite, returning it with the raw token exactly once.

    Raises ``ValueError`` for an address the waitlist would refuse; the controllers
    translate that into their audience's 400."""
    normalized = normalize_waitlist_address(email)
    if normalized is None:
        raise ValueError(INVALID_ADDRESS_MESSAGE)
    raw = mint_invite_token()
    invite = Invite(
        token_hash=token_hash(raw),
        email=normalized,
        invited_by=invited_by,
        expires_at=now + INVITE_TTL,
    )
    db_session.add(invite)
    await db_session.commit()
    return invite, raw


async def revoke_invite(
    db_session: AsyncSession, invite_id: UUID, now: datetime
) -> Invite | None:
    """Stamp an invite revoked. Idempotent: a second revoke keeps the first stamp, and
    revoking a redeemed invite is how an alpha member loses access."""
    invite = await db_session.get(Invite, invite_id)
    if invite is None:
        return None
    if invite.revoked_at is None:
        invite.revoked_at = now
        await db_session.commit()
    return invite


async def find_by_token(db_session: AsyncSession, raw: str) -> Invite | None:
    result = await db_session.execute(
        select(Invite).where(Invite.token_hash == token_hash(raw))
    )
    return result.scalar_one_or_none()


async def all_invites(db_session: AsyncSession) -> list[Invite]:
    """Every invite, newest first — what the admin page lists."""
    result = await db_session.execute(
        select(Invite).order_by(Invite.created_at.desc())
    )
    return list(result.scalars())
