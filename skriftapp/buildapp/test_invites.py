"""The invite domain, with no web stack in sight: what a minted token looks like, the
precedence the five states resolve in, and the two rules a redemption must satisfy —
the invite is open, and the account's address is the one it was sent to."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from uuid import uuid4

import pytest

from buildapp import invites
from buildapp.invites import (
    EMAIL_MISMATCH,
    INVITE_TTL,
    InviteState,
    invite_state,
    invite_url,
    mint_invite_token,
    redeem,
)
from buildapp.models import Invite
from buildapp.token_hash import token_hash

NOW = datetime(2026, 9, 6, 12, 0, tzinfo=timezone.utc)
INVITED = "invitee@example.com"


def invite(**fields) -> Invite:
    defaults = dict(
        token_hash=token_hash("inv_raw"),
        email=INVITED,
        invited_by=None,
        expires_at=NOW + INVITE_TTL,
        redeemed_by=None,
        redeemed_at=None,
        revoked_at=None,
    )
    defaults.update(fields)
    return Invite(**defaults)


def test_a_minted_token_is_prefixed_and_stored_only_as_its_hash():
    raw = mint_invite_token()
    assert raw.startswith("inv_")
    assert len(raw) > len("inv_") + 20
    assert mint_invite_token() != raw
    assert token_hash(raw) != raw and len(token_hash(raw)) == 64


def test_the_invite_url_is_the_public_base_plus_the_raw_token():
    assert invite_url("https://getbuild.ing", "inv_abc") == (
        "https://getbuild.ing/invite/inv_abc"
    )


def test_an_absent_invite_is_unknown():
    assert invite_state(None, NOW) is InviteState.UNKNOWN


def test_state_precedence_is_unknown_revoked_redeemed_expired_open():
    revoked_and_redeemed = invite(
        revoked_at=NOW, redeemed_at=NOW, redeemed_by=uuid4(), expires_at=NOW
    )
    assert invite_state(revoked_and_redeemed, NOW) is InviteState.REVOKED
    redeemed_and_expired = invite(redeemed_at=NOW, redeemed_by=uuid4(), expires_at=NOW)
    assert invite_state(redeemed_and_expired, NOW) is InviteState.REDEEMED
    assert invite_state(invite(expires_at=NOW), NOW) is InviteState.EXPIRED
    assert invite_state(invite(), NOW) is InviteState.OPEN


def test_an_invite_expires_exactly_fourteen_days_after_it_was_issued():
    assert INVITE_TTL == timedelta(days=14)
    issued = invite(expires_at=NOW + INVITE_TTL)
    assert invite_state(issued, NOW + INVITE_TTL - timedelta(seconds=1)) is InviteState.OPEN
    assert invite_state(issued, NOW + INVITE_TTL) is InviteState.EXPIRED


@pytest.mark.parametrize(
    ("fields", "reason"),
    [
        ({"revoked_at": NOW}, InviteState.REVOKED),
        ({"redeemed_at": NOW, "redeemed_by": uuid4()}, InviteState.REDEEMED),
        ({"expires_at": NOW}, InviteState.EXPIRED),
    ],
)
def test_redeem_refuses_every_non_open_state_without_touching_the_row(fields, reason):
    row = invite(**fields)
    before = (row.redeemed_by, row.redeemed_at)
    outcome = redeem(row, uuid4(), INVITED, NOW)
    assert outcome.ok is False
    assert outcome.reason is reason
    assert (row.redeemed_by, row.redeemed_at) == before


def test_redeem_refuses_an_account_that_is_not_the_invited_address():
    row = invite()
    outcome = redeem(row, uuid4(), "someone.else@example.com", NOW)
    assert outcome.ok is False
    assert outcome.reason is EMAIL_MISMATCH
    assert row.redeemed_by is None


def test_redeem_compares_the_account_address_normalized():
    row = invite()
    user_id = uuid4()
    outcome = redeem(row, user_id, "  Invitee@Example.COM ", NOW)
    assert outcome.ok is True
    assert row.redeemed_by == user_id


def test_redeem_marks_the_row_redeemed_once_and_then_refuses():
    row = invite()
    user_id = uuid4()
    assert redeem(row, user_id, INVITED, NOW).ok is True
    assert (row.redeemed_by, row.redeemed_at) == (user_id, NOW)
    later = redeem(row, uuid4(), INVITED, NOW + timedelta(minutes=1))
    assert later.ok is False
    assert later.reason is InviteState.REDEEMED
    assert row.redeemed_by == user_id


@pytest.mark.asyncio
async def test_issue_invite_stores_the_hash_normalizes_the_address_and_dates_the_expiry(db):
    inviter = uuid4()
    row, raw = await invites.issue_invite(db, "  Invitee@Example.COM ", inviter, NOW)
    assert raw.startswith("inv_")
    assert row.email == INVITED
    assert row.token_hash == token_hash(raw)
    assert row.invited_by == inviter
    assert row.expires_at == NOW + INVITE_TTL
    assert await invites.find_by_token(db, raw) is not None


@pytest.mark.asyncio
async def test_issue_invite_refuses_an_address_the_waitlist_would_refuse(db):
    with pytest.raises(ValueError):
        await invites.issue_invite(db, "not-an-address", uuid4(), NOW)


@pytest.mark.asyncio
async def test_find_by_token_answers_nothing_for_a_token_never_issued(db):
    await invites.issue_invite(db, INVITED, uuid4(), NOW)
    assert await invites.find_by_token(db, "inv_never-issued") is None


@pytest.mark.asyncio
async def test_revoke_stamps_the_row_and_is_idempotent(db):
    row, _ = await invites.issue_invite(db, INVITED, uuid4(), NOW)
    await invites.revoke_invite(db, row.id, NOW)
    assert row.revoked_at == NOW
    await invites.revoke_invite(db, row.id, NOW + timedelta(days=1))
    assert row.revoked_at == NOW


@pytest.mark.asyncio
async def test_revoking_an_invite_that_does_not_exist_is_quiet(db):
    await invites.revoke_invite(db, uuid4(), NOW)


def test_redeem_matches_an_address_the_waitlist_itself_would_refuse():
    """A seeded invite (the dev stack's, for qa@localhost) must still be redeemable by
    that account: matching two addresses is about identity, not deliverability."""
    row = invite(email="qa@localhost")
    user_id = uuid4()
    assert redeem(row, user_id, " QA@Localhost ", NOW).ok is True
    assert row.redeemed_by == user_id


def test_redeem_still_refuses_a_different_address_of_that_shape():
    row = invite(email="qa@localhost")
    assert redeem(row, uuid4(), "someone@localhost", NOW).reason is EMAIL_MISMATCH
