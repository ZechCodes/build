"""Alpha membership has one home and one table of rules: a redeemed, unrevoked invite,
or the administrator permission. This pins both — including that revoking a redeemed
invite is how a member is removed — and the 403 the guards raise for everyone else."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from uuid import uuid4

import pytest
from litestar.exceptions import PermissionDeniedException

from buildapp import invites
from buildapp.alpha_membership import (
    INVITE_ONLY_DETAIL,
    MEMBERSHIP_RULES,
    holds_a_redeemed_invite,
    is_alpha_member,
    is_an_administrator,
    require_alpha_member,
)
from buildapp.db_test_support import MEMBER_ADDRESS, add_account, add_member

NOW = datetime(2026, 9, 6, 12, 0, tzinfo=timezone.utc)


@pytest.mark.asyncio
async def test_a_redeemed_unrevoked_invite_makes_its_account_a_member(db):
    user_id = uuid4()
    await add_member(db, user_id)
    assert await is_alpha_member(db, user_id) is True


@pytest.mark.asyncio
async def test_revoking_the_redeemed_invite_removes_the_member(db):
    user_id = uuid4()
    invite = await add_member(db, user_id)
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
    await add_member(db, user_id)
    assert await require_alpha_member(db, user_id) is None


@pytest.mark.asyncio
async def test_require_alpha_member_refuses_everyone_else_with_invite_only(db):
    with pytest.raises(PermissionDeniedException) as refusal:
        await require_alpha_member(db, uuid4())
    assert refusal.value.detail == INVITE_ONLY_DETAIL
    assert INVITE_ONLY_DETAIL == "invite only"
    assert refusal.value.status_code == 403


@pytest.mark.asyncio
async def test_an_administrator_is_a_member_without_an_invite(db):
    admin_id = await add_account(db, "operator@example.com", administrator=True)
    assert await is_alpha_member(db, admin_id) is True
    assert await require_alpha_member(db, admin_id) is None


@pytest.mark.asyncio
async def test_an_ordinary_account_with_no_invite_is_still_refused(db):
    user_id = await add_account(db, "someone@example.com")
    assert await is_alpha_member(db, user_id) is False


def test_the_ways_in_are_a_table_the_invite_rule_leads():
    # A new way into the alpha is a new entry here, never a branch in a gate.
    assert MEMBERSHIP_RULES == (holds_a_redeemed_invite, is_an_administrator)
