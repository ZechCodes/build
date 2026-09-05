"""Who is in the alpha. One fact, one home.

An account is a member iff one of ``MEMBERSHIP_RULES`` says so: it holds a redeemed
invite that has not been revoked — the redeemed invite IS the marker, so there is no
members table to keep in step and revoking a redeemed invite is the operator's "remove
member" action — or it is an administrator, so the operator is never locked out of the
app they administer. A new way in is a new rule in the table, not a new branch.

Every gate reads this module: ``desktop_auth.build_auth_guard`` (and so every route
carrying it, device approval included), ``BuildController.index`` and ``/app/downloads``.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from uuid import UUID

from litestar.exceptions import PermissionDeniedException
from skrift.auth.services import get_user_permissions
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.models import Invite

INVITE_ONLY_DETAIL = "invite only"
ADMINISTRATOR_PERMISSION = "administrator"

MembershipRule = Callable[[AsyncSession, UUID], Awaitable[bool]]


async def holds_a_redeemed_invite(db_session: AsyncSession, user_id: UUID) -> bool:
    result = await db_session.execute(
        select(Invite.id)
        .where(Invite.redeemed_by == user_id, Invite.revoked_at.is_(None))
        .limit(1)
    )
    return result.first() is not None


async def is_an_administrator(db_session: AsyncSession, user_id: UUID) -> bool:
    permissions = await get_user_permissions(db_session, user_id)
    return ADMINISTRATOR_PERMISSION in permissions.permissions


#: The ways into the alpha, asked in order until one says yes.
MEMBERSHIP_RULES: tuple[MembershipRule, ...] = (
    holds_a_redeemed_invite,
    is_an_administrator,
)


async def is_alpha_member(db_session: AsyncSession, user_id: UUID) -> bool:
    for rule in MEMBERSHIP_RULES:
        if await rule(db_session, user_id):
            return True
    return False


async def require_alpha_member(db_session: AsyncSession, user_id: UUID) -> None:
    if not await is_alpha_member(db_session, user_id):
        raise PermissionDeniedException(INVITE_ONLY_DETAIL)
