"""Open links bind on redemption, including competing requests holding stale rows."""
from datetime import timedelta
from uuid import uuid4

import pytest

from buildapp import invites
from buildapp.db_test_support import create_skrift_tables, engine_for, file_session_maker
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
