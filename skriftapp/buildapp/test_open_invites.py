"""Open links bind on redemption, including competing requests holding stale rows."""
import asyncio
from datetime import timedelta
from uuid import uuid4

import pytest

from buildapp import invites
from buildapp.db_test_support import add_account, create_skrift_tables, engine_for, file_session_maker
from buildapp.models import Invite
from buildapp.test_invites import NOW, invite


@pytest.mark.asyncio
async def test_an_open_link_has_no_address_and_keeps_only_the_token_hash(db):
    row, raw = await invites.issue_open_invite(db, None, NOW)
    assert row.kind == "open_link"
    assert row.email == ""
    assert row.token_hash != raw
    assert row.expires_at == NOW + invites.INVITE_TTL
    assert (await invites.find_by_token(db, raw)).id == row.id


@pytest.mark.parametrize("email", [" Visitor@Example.COM ", "qa@localhost"])
def test_open_link_binds_to_the_accounts_canonical_address_once(email):
    row = invite(kind="open_link", email="")
    user_id = uuid4()
    assert invites.redeem(row, user_id, email, NOW).ok
    assert row.email == email.strip().lower()
    assert row.redeemed_by == user_id
    assert not invites.redeem(row, uuid4(), "another@example.com", NOW).ok
    assert row.email == email.strip().lower()


def test_an_open_link_cannot_redeem_for_an_account_without_an_address():
    row = invite(kind="open_link", email="")
    assert not invites.redeem(row, uuid4(), "", NOW).ok
    assert row.redeemed_by is None


@pytest.mark.asyncio
async def test_existing_invites_default_to_email_bound(db):
    row, _ = await invites.issue_invite(db, "bound@example.com", None, NOW)
    assert row.kind == "email_bound"
    assert not invites.redeem(row, uuid4(), "other@example.com", NOW).ok


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["email_bound", "open_link"])
async def test_only_one_stale_request_can_claim_an_invite(tmp_path, kind):
    sessions = file_session_maker(tmp_path / "race.db")
    engine = engine_for(sessions)
    await create_skrift_tables(engine)
    try:
        async with sessions() as first, sessions() as second:
            if kind == "open_link":
                row, _ = await invites.issue_open_invite(first, None, NOW)
            else:
                row, _ = await invites.issue_invite(first, "first@example.com", None, NOW)
            stale = await second.get(Invite, row.id)
            first_user, second_user = uuid4(), uuid4()
            assert (await invites.claim_invite(first, row, first_user, "first@example.com", NOW)).ok
            await first.commit()
            assert not (await invites.claim_invite(second, stale, second_user, "first@example.com", NOW)).ok
            await second.commit()
            await second.refresh(stale)
            assert stale.redeemed_by == first_user
            assert stale.email == "first@example.com"
    finally:
        await engine.dispose()


@pytest.mark.asyncio
@pytest.mark.parametrize("spoiled", ["revoked", "expired", "redeemed"])
async def test_claim_rechecks_database_state_before_binding(db, spoiled):
    row, _ = await invites.issue_open_invite(db, None, NOW)
    if spoiled == "revoked":
        row.revoked_at = NOW
    elif spoiled == "expired":
        row.expires_at = NOW - timedelta(seconds=1)
    else:
        row.email = "first@example.com"
        row.redeemed_by, row.redeemed_at = uuid4(), NOW
    await db.commit()
    assert not (await invites.claim_invite(db, row, uuid4(), "second@example.com", NOW)).ok
    assert row.email != "second@example.com"


@pytest.mark.asyncio
async def test_two_concurrent_links_cannot_grant_one_account_two_memberships(tmp_path, monkeypatch):
    sessions = file_session_maker(tmp_path / "membership-race.db")
    engine = engine_for(sessions)
    await create_skrift_tables(engine)
    checked = asyncio.Event()
    checks = 0
    holds_invite = invites.holds_a_redeemed_invite

    async def both_check_before_claiming(session, user_id):
        nonlocal checks
        result = await holds_invite(session, user_id)
        checks += 1
        if checks == 2:
            checked.set()
        await asyncio.wait_for(checked.wait(), timeout=5)
        return result

    monkeypatch.setattr(invites, "holds_a_redeemed_invite", both_check_before_claiming)
    try:
        async with sessions() as setup:
            user_id = await add_account(setup, "member@example.com")
            first, _ = await invites.issue_open_invite(setup, None, NOW)
            second, _ = await invites.issue_open_invite(setup, None, NOW)

        async def claim(invite_id):
            async with sessions() as session:
                row = await session.get(Invite, invite_id)
                result = await invites.claim_invite(session, row, user_id, "member@example.com", NOW)
                await session.commit()
                return result

        results = await asyncio.gather(claim(first.id), claim(second.id))
        assert sum(result.ok for result in results) == 1
        assert [result.reason for result in results if not result.ok] == [invites.ALREADY_MEMBER]
        async with sessions() as session:
            rows = await invites.all_invites(session)
            assert sum(row.redeemed_by == user_id for row in rows) == 1
            assert sum(row.email == "" for row in rows) == 1
    finally:
        await engine.dispose()
