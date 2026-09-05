"""Who is in the alpha. One fact, one home.

An account is a member iff it holds a redeemed invite that has not been revoked — the
redeemed invite IS the marker, so there is no members table to keep in step and
revoking a redeemed invite is the operator's "remove member" action. Administrators are
not implicit members: the operator invites their own address like anyone else.

Every gate reads this module: ``desktop_auth.build_auth_guard`` (and so every route
carrying it, device approval included), ``BuildController.index`` and ``/app/downloads``.
"""

from __future__ import annotations

from uuid import UUID

from litestar.exceptions import PermissionDeniedException
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.models import Invite

INVITE_ONLY_DETAIL = "invite only"


async def is_alpha_member(db_session: AsyncSession, user_id: UUID) -> bool:
    result = await db_session.execute(
        select(Invite.id)
        .where(Invite.redeemed_by == user_id, Invite.revoked_at.is_(None))
        .limit(1)
    )
    return result.first() is not None


async def require_alpha_member(db_session: AsyncSession, user_id: UUID) -> None:
    if not await is_alpha_member(db_session, user_id):
        raise PermissionDeniedException(INVITE_ONLY_DETAIL)
