"""Read Skrift account addresses and Build-owned account policy.

One home for "what address is this account?", because three callers ask: the invite
redemption (which must compare it to the invited address), the SPA shell (which shows
its initial, and names it on the invite-only page), and the admin invites page (which
names the account behind every id in the table at once)."""

from __future__ import annotations

from dataclasses import dataclass
from uuid import UUID

from skrift.db.models.user import User
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.models import AccountPolicy


@dataclass(frozen=True)
class AccountPolicyValues:
    account_type: str = "free"
    tunnel_access: bool = False
    promotion_revision: int = 0


async def account_policy(db_session: AsyncSession, user_id: UUID) -> AccountPolicyValues:
    """A missing policy row has the same defaults for old and newly created users."""
    result = await db_session.execute(
        select(
            AccountPolicy.account_type, AccountPolicy.tunnel_access, AccountPolicy.promotion_revision
        ).where(AccountPolicy.id == user_id)
    )
    row = result.one_or_none()
    return AccountPolicyValues(*row) if row is not None else AccountPolicyValues()


async def account_email(db_session: AsyncSession, user_id: UUID) -> str:
    """The account's address, or the empty string when it has none — which no
    normalized invite address ever equals, so a redemption simply refuses."""
    result = await db_session.execute(select(User.email).where(User.id == user_id))
    return result.scalar_one_or_none() or ""


async def addresses_by_id(db_session: AsyncSession) -> dict[UUID, str]:
    """Every account that has an address, keyed by id — one read for a page that names
    the account behind a column of ids."""
    result = await db_session.execute(select(User.id, User.email))
    return {user_id: email for user_id, email in result if email}
