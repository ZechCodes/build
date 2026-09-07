"""The download token — what the install one-liner carries instead of a session.

``curl … | sh`` runs with no browser and no cookies, so the onboarding page mints one
of these into the line the human copies. It is a member's identity for ten minutes and
for one install: the token names the account it was minted for, the guard still asks
whether that account is an alpha member (a revoked invite kills a live token), and the
tarball fetch — the last of the three the script makes — deletes it.

The row is the shared ``EphemeralToken`` under purpose ``download``; the minting and
lookup rules are ``ephemeral_tokens``'. What is this module's own is the shape, the
lifetime, and what spending means.
"""

from __future__ import annotations

import re
from datetime import datetime, timedelta
from uuid import UUID

from sqlalchemy import delete
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import ephemeral_tokens
from buildapp.models import EphemeralToken
from buildapp.token_hash import token_hash

PURPOSE = "download"
PREFIX = "dl_"
#: ``secrets.token_urlsafe(24)`` is always 32 url-safe characters. The token is
#: substituted into a shell script and into a URL, so nothing but this charset may
#: pass — and nothing else in the app judges the shape.
TOKEN_PATTERN = re.compile(r"^dl_[A-Za-z0-9_-]{32}$")
TTL = timedelta(minutes=10)
EXPIRES_IN_S = int(TTL.total_seconds())


def is_well_formed(raw: str | None) -> bool:
    return bool(raw) and TOKEN_PATTERN.fullmatch(raw) is not None


async def mint(db_session: AsyncSession, user_id: UUID, now: datetime) -> str:
    """A fresh ten-minute token for this member. Every render of the downloads page
    mints one, so the line on screen is always the freshest one."""
    return await ephemeral_tokens.mint(
        db_session,
        purpose=PURPOSE,
        prefix=PREFIX,
        user_id=user_id,
        ttl=TTL,
        now=now,
    )


async def holder(
    db_session: AsyncSession, raw: str | None, now: datetime
) -> UUID | None:
    """The member a live token belongs to, or ``None``. The shape is checked before
    any database read, so junk in a query string costs nothing."""
    if not is_well_formed(raw):
        return None
    return await ephemeral_tokens.holder_of(
        db_session, purpose=PURPOSE, raw=raw, now=now
    )


async def spend(db_session: AsyncSession, raw: str | None, user_id: UUID) -> None:
    """Burn the token: one install line, one install. Holder-only — a member browsing
    with a session never burns the token another install is carrying — and a no-op for
    a missing, malformed or unknown one."""
    if not is_well_formed(raw):
        return
    await db_session.execute(
        delete(EphemeralToken).where(
            EphemeralToken.token_hash == token_hash(raw),
            EphemeralToken.purpose == PURPOSE,
            EphemeralToken.user_id == user_id,
        )
    )
    await db_session.commit()
