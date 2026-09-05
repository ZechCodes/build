"""Alpha membership has one home and one definition: a redeemed, unrevoked invite. This
pins that definition — including that revoking a redeemed invite is how a member is
removed — and the 403 the guards raise for everyone else."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from uuid import uuid4

import pytest
from litestar.exceptions import PermissionDeniedException

from buildapp import invites
from buildapp.alpha_membership import (
    INVITE_ONLY_DETAIL,
    is_alpha_member,
    require_alpha_member,
)
from buildapp.models import Invite

NOW = datetime(2026, 9, 6, 12, 0, tzinfo=timezone.utc)
MEMBER_ADDRESS = "member@example.com"


async def redeemed_invite(db, user_id) -> Invite:
    invite, _ = await invites.issue_invite(db, MEMBER_ADDRESS, uuid4(), NOW)
    assert invites.redeem(invite, user_id, MEMBER_ADDRESS, NOW).ok is True
    await db.commit()
    return invite


@pytest.mark.asyncio
async def test_a_redeemed_unrevoked_invite_makes_its_account_a_member(db):
    user_id = uuid4()
    await redeemed_invite(db, user_id)
    assert await is_alpha_member(db, user_id) is True


@pytest.mark.asyncio
async def test_revoking_the_redeemed_invite_removes_the_member(db):
    user_id = uuid4()
    invite = await redeemed_invite(db, user_id)
    await invites.revoke_invite(db, invite.id, NOW + timedelta(days=1))
    assert await is_alpha_member(db, user_id) is False


@pytest.mark.asyncio
async def test_an_open_invite_confers_nothing_until_it_is_redeemed(db):
    await invites.issue_invite(db, MEMBER_ADDRESS, uuid4(), NOW)
    assert await is_alpha_member(db, uuid4()) is False


@pytest.mark.asyncio
async def test_an_expired_invite_the_account_never_redeemed_confers_nothing(db):
    invite, _ = await invites.issue_invite(db, MEMBER_ADDRESS, uuid4(), NOW)
    invite.expires_at = NOW - timedelta(days=1)
    await db.commit()
    assert await is_alpha_member(db, uuid4()) is False


@pytest.mark.asyncio
async def test_an_account_with_no_invite_at_all_is_not_a_member(db):
    assert await is_alpha_member(db, uuid4()) is False


@pytest.mark.asyncio
async def test_require_alpha_member_passes_a_member_through(db):
    user_id = uuid4()
    await redeemed_invite(db, user_id)
    assert await require_alpha_member(db, user_id) is None


@pytest.mark.asyncio
async def test_require_alpha_member_refuses_everyone_else_with_invite_only(db):
    with pytest.raises(PermissionDeniedException) as refusal:
        await require_alpha_member(db, uuid4())
    assert refusal.value.detail == INVITE_ONLY_DETAIL
    assert INVITE_ONLY_DETAIL == "invite only"
    assert refusal.value.status_code == 403
