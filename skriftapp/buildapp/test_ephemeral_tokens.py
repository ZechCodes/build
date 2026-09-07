"""One minting rule for every short-lived token this app hands out.

The gateway token taught it: mint a random raw string, store only its SHA-256, stamp a
purpose and an expiry, and purge whatever has already expired while you are there. The
download token reuses that rule rather than copying it, so a change to how a token is
stored is one edit, not two."""

from __future__ import annotations

from datetime import timedelta
from uuid import uuid4

import pytest
from sqlalchemy import select

from buildapp import ephemeral_tokens
from buildapp.clock import utc_now
from buildapp.db_test_support import add_account
from buildapp.models import EphemeralToken
from buildapp.token_hash import token_hash

TTL = timedelta(minutes=5)
PURPOSE = "gateway"
PREFIX = "gw_"


async def _rows(db) -> list[EphemeralToken]:
    return list((await db.execute(select(EphemeralToken))).scalars().all())


@pytest.mark.asyncio
async def test_a_minted_token_is_stored_as_its_hash_and_nothing_else(db):
    user_id = await add_account(db, "holder@example.com")
    now = utc_now()

    raw = await ephemeral_tokens.mint(
        db, purpose=PURPOSE, prefix=PREFIX, user_id=user_id, ttl=TTL, now=now
    )

    assert raw.startswith(PREFIX)
    (row,) = await _rows(db)
    assert row.token_hash == token_hash(raw)
    assert row.purpose == PURPOSE
    assert row.user_id == user_id
    assert row.expires_at == now + TTL
    assert raw not in str(row.__dict__)


@pytest.mark.asyncio
async def test_every_mint_is_a_different_token(db):
    user_id = await add_account(db, "holder@example.com")
    first, second = [
        await ephemeral_tokens.mint(
            db, purpose=PURPOSE, prefix=PREFIX, user_id=user_id, ttl=TTL, now=utc_now()
        )
        for _ in range(2)
    ]
    assert first != second


@pytest.mark.asyncio
async def test_minting_purges_expired_rows_of_every_purpose(db):
    """Nothing else deletes these rows, so without this the table grows forever —
    and an expired row of another purpose is just as dead."""
    user_id = await add_account(db, "holder@example.com")
    now = utc_now()
    db.add_all(
        [
            EphemeralToken(
                token_hash=token_hash(f"stale-{purpose}"),
                purpose=purpose,
                user_id=user_id,
                expires_at=now - timedelta(seconds=1),
            )
            for purpose in ("gateway", "download")
        ]
    )
    await db.commit()

    raw = await ephemeral_tokens.mint(
        db, purpose=PURPOSE, prefix=PREFIX, user_id=user_id, ttl=TTL, now=now
    )

    assert [row.token_hash for row in await _rows(db)] == [token_hash(raw)]


@pytest.mark.asyncio
async def test_a_live_row_of_another_purpose_survives_a_mint(db):
    user_id = await add_account(db, "holder@example.com")
    now = utc_now()
    other = await ephemeral_tokens.mint(
        db, purpose="download", prefix="dl_", user_id=user_id, ttl=TTL, now=now
    )
    await ephemeral_tokens.mint(
        db, purpose=PURPOSE, prefix=PREFIX, user_id=user_id, ttl=TTL, now=now
    )
    assert (
        await ephemeral_tokens.holder_of(db, purpose="download", raw=other, now=now)
        == user_id
    )


@pytest.mark.asyncio
async def test_the_holder_of_a_live_token_is_the_account_it_was_minted_for(db):
    user_id = await add_account(db, "holder@example.com")
    now = utc_now()
    raw = await ephemeral_tokens.mint(
        db, purpose=PURPOSE, prefix=PREFIX, user_id=user_id, ttl=TTL, now=now
    )
    assert (
        await ephemeral_tokens.holder_of(db, purpose=PURPOSE, raw=raw, now=now)
        == user_id
    )


@pytest.mark.asyncio
async def test_a_token_minted_for_another_purpose_names_nobody(db):
    """A gateway token must not open a download, and the reverse."""
    user_id = await add_account(db, "holder@example.com")
    now = utc_now()
    raw = await ephemeral_tokens.mint(
        db, purpose=PURPOSE, prefix=PREFIX, user_id=user_id, ttl=TTL, now=now
    )
    assert (
        await ephemeral_tokens.holder_of(db, purpose="download", raw=raw, now=now)
        is None
    )


@pytest.mark.asyncio
async def test_an_expired_token_names_nobody(db):
    user_id = await add_account(db, "holder@example.com")
    now = utc_now()
    raw = await ephemeral_tokens.mint(
        db, purpose=PURPOSE, prefix=PREFIX, user_id=user_id, ttl=TTL, now=now
    )
    later = now + TTL + timedelta(seconds=1)
    assert (
        await ephemeral_tokens.holder_of(db, purpose=PURPOSE, raw=raw, now=later)
        is None
    )


@pytest.mark.asyncio
async def test_a_token_that_was_never_minted_names_nobody(db):
    await add_account(db, "holder@example.com")
    assert (
        await ephemeral_tokens.holder_of(
            db, purpose=PURPOSE, raw=f"{PREFIX}{uuid4().hex}", now=utc_now()
        )
        is None
    )
