"""Minting and reading the short-lived tokens this app hands out.

One rule, learned from the browser→relay gateway token and now shared with the
download token: the raw string is returned to the caller once and never stored — only
its SHA-256 is — under a purpose, an owner and an expiry. Minting also purges whatever
has already expired, because nothing else ever deletes these rows.

A purpose is a namespace: a token minted for one purpose names nobody under another.
"""

from __future__ import annotations

import secrets
from datetime import datetime, timedelta
from uuid import UUID

from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.models import EphemeralToken
from buildapp.token_hash import token_hash

#: How much entropy a raw token carries after its prefix: 24 bytes → 32 url-safe chars.
TOKEN_BYTES = 24


async def mint(
    db_session: AsyncSession,
    *,
    purpose: str,
    prefix: str,
    user_id: UUID,
    ttl: timedelta,
    now: datetime,
) -> str:
    """A fresh token for this account, stored as its hash and returned raw once."""
    await db_session.execute(
        delete(EphemeralToken).where(EphemeralToken.expires_at < now)
    )
    raw = prefix + secrets.token_urlsafe(TOKEN_BYTES)
    db_session.add(
        EphemeralToken(
            token_hash=token_hash(raw),
            purpose=purpose,
            user_id=user_id,
            expires_at=now + ttl,
        )
    )
    await db_session.commit()
    return raw


async def holder_of(
    db_session: AsyncSession, *, purpose: str, raw: str, now: datetime
) -> UUID | None:
    """The account a live token of this purpose was minted for, or ``None``."""
    row = (
        await db_session.execute(
            select(EphemeralToken).where(
                EphemeralToken.token_hash == token_hash(raw),
                EphemeralToken.purpose == purpose,
                EphemeralToken.expires_at > now,
            )
        )
    ).scalar_one_or_none()
    return row.user_id if row is not None else None
