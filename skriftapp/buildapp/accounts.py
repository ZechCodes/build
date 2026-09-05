"""The account record behind a signed-in user, read as columns rather than as an ORM
entity so a caller needs only the users table — no roles, no relationships.

One home for "what address is this account?", because two callers ask: the invite
redemption (which must compare it to the invited address) and the SPA shell (which
shows its initial, and names it on the invite-only page)."""

from __future__ import annotations

from uuid import UUID

from skrift.db.models.user import User
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession


async def account_email(db_session: AsyncSession, user_id: UUID) -> str:
    """The account's address, or the empty string when it has none — which no
    normalized invite address ever equals, so a redemption simply refuses."""
    result = await db_session.execute(select(User.email).where(User.id == user_id))
    return result.scalar_one_or_none() or ""
