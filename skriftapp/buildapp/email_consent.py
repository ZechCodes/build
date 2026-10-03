"""Account-level consent for optional Build product email.

Invite and waitlist messages are transactional. Any future product-mail sender must
obtain recipients through ``consenting_product_email_addresses`` so a missing row,
unchecked box, or missing opt-in timestamp never becomes implied consent.
"""

from __future__ import annotations

from collections.abc import Iterable
from datetime import datetime
from uuid import UUID

from skrift.db.models.user import User
from sqlalchemy import func, select, update
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.models import UserEmailPreference
from buildapp.waitlist_address import canonical_address


def record_signup_consent(
    db_session: AsyncSession, user_id: UUID, opt_in: bool, now: datetime
) -> None:
    """Stage a new account's explicit signup choice in the redemption transaction."""
    db_session.add(
        UserEmailPreference(
            user_id=user_id,
            product_email_opt_in=opt_in,
            product_email_opted_in_at=now if opt_in else None,
        )
    )


async def consent_by_user_id(
    db_session: AsyncSession, user_ids: Iterable[UUID]
) -> dict[UUID, bool]:
    """Consent status for a displayed set of users; absent means false."""
    wanted = set(user_ids)
    if not wanted:
        return {}
    rows = await db_session.execute(
        select(
            UserEmailPreference.user_id,
            UserEmailPreference.product_email_opt_in,
            UserEmailPreference.product_email_opted_in_at,
        ).where(UserEmailPreference.user_id.in_(wanted))
    )
    recorded = {
        user_id: bool(opt_in and opted_in_at)
        for user_id, opt_in, opted_in_at in rows
    }
    return {user_id: recorded.get(user_id, False) for user_id in wanted}


async def consenting_product_email_addresses(db_session: AsyncSession) -> list[str]:
    """The sole recipient query for future optional product mail."""
    rows = await db_session.execute(
        select(User.email)
        .join(UserEmailPreference, UserEmailPreference.user_id == User.id)
        .where(
            UserEmailPreference.product_email_opt_in.is_(True),
            UserEmailPreference.product_email_opted_in_at.is_not(None),
            User.email.is_not(None),
            User.is_active.is_(True),
        )
        .order_by(User.email)
    )
    return [email for email in rows.scalars() if email]


async def revoke_consent_for_email(db_session: AsyncSession, email: str) -> None:
    """A waitlist unsubscribe also withdraws optional product-email consent for any
    registered account with the same canonical address."""
    canonical = canonical_address(email)
    if not canonical:
        return
    user_ids = select(User.id).where(func.lower(func.trim(User.email)) == canonical)
    await db_session.execute(
        update(UserEmailPreference)
        .where(UserEmailPreference.user_id.in_(user_ids))
        .values(product_email_opt_in=False, product_email_opted_in_at=None)
    )
