"""The download token: what a member's install one-liner carries instead of a browser
session.

Ten minutes, bound to the account it was minted for, and spent — deleted — by the one
fetch that matters. Stored the way every token here is stored: only the hash."""

from __future__ import annotations

import re
from datetime import timedelta

import pytest
from sqlalchemy import select

from buildapp import download_tokens, ephemeral_tokens
from buildapp.clock import utc_now
from buildapp.db_test_support import add_account
from buildapp.models import EphemeralToken
from buildapp.token_hash import token_hash


async def _rows(db) -> list[EphemeralToken]:
    return list((await db.execute(select(EphemeralToken))).scalars().all())


def test_the_token_is_a_prefixed_thirty_two_character_url_safe_string():
    """The token is substituted into a shell script and into a URL, so its charset is
    pinned here and nowhere else judges it."""
    assert download_tokens.TOKEN_PATTERN.pattern == r"^dl_[A-Za-z0-9_-]{32}$"
    assert download_tokens.TTL == timedelta(minutes=10)
    assert download_tokens.EXPIRES_IN_S == 600


@pytest.mark.parametrize(
    "raw",
    [
        "",
        "dl_",
        "gw_" + "x" * 32,
        "dl_" + "x" * 31,
        "dl_" + "x" * 33,
        "dl_" + "x" * 30 + " y",
        "dl_" + "x" * 30 + "$yz",
        "dl_" + "x" * 32 + "\n",
    ],
)
def test_only_the_exact_shape_is_well_formed(raw):
    assert not download_tokens.is_well_formed(raw)


@pytest.mark.asyncio
async def test_a_minted_token_is_well_formed_and_stored_only_as_its_hash(db):
    user_id = await add_account(db, "member@example.com")
    now = utc_now()

    raw = await download_tokens.mint(db, user_id, now)

    assert download_tokens.is_well_formed(raw)
    assert re.fullmatch(r"dl_[A-Za-z0-9_-]{32}", raw)
    (row,) = await _rows(db)
    assert row.token_hash == token_hash(raw)
    assert row.purpose == download_tokens.PURPOSE == "download"
    assert row.user_id == user_id
    assert row.expires_at == now + download_tokens.TTL


@pytest.mark.asyncio
async def test_the_holder_is_the_member_it_was_minted_for_for_ten_minutes(db):
    user_id = await add_account(db, "member@example.com")
    now = utc_now()
    raw = await download_tokens.mint(db, user_id, now)

    assert await download_tokens.holder(db, raw, now) == user_id
    assert (
        await download_tokens.holder(db, raw, now + timedelta(minutes=9, seconds=59))
        == user_id
    )


@pytest.mark.asyncio
async def test_nobody_holds_an_expired_token(db):
    user_id = await add_account(db, "member@example.com")
    now = utc_now()
    raw = await download_tokens.mint(db, user_id, now)
    assert await download_tokens.holder(db, raw, now + timedelta(minutes=10)) is None


@pytest.mark.asyncio
async def test_nobody_holds_a_token_that_was_never_minted(db):
    await add_account(db, "member@example.com")
    unminted = "dl_" + "x" * 32
    assert await download_tokens.holder(db, unminted, utc_now()) is None


@pytest.mark.asyncio
async def test_a_malformed_token_never_reaches_the_database():
    """The shape is checked before the query, so a stream of junk in a query string
    costs no database work at all."""

    class RefusingSession:
        async def execute(self, *_args, **_kwargs):
            raise AssertionError("a malformed token must not be looked up")

    assert (
        await download_tokens.holder(RefusingSession(), "not-a-token", utc_now())
        is None
    )
    assert await download_tokens.holder(RefusingSession(), None, utc_now()) is None


@pytest.mark.asyncio
async def test_spending_a_token_deletes_it_so_it_opens_nothing_twice(db):
    user_id = await add_account(db, "member@example.com")
    now = utc_now()
    raw = await download_tokens.mint(db, user_id, now)

    await download_tokens.spend(db, raw, user_id)

    assert await _rows(db) == []
    assert await download_tokens.holder(db, raw, now) is None


@pytest.mark.asyncio
async def test_another_account_cannot_spend_a_token_it_does_not_hold(db):
    """Spending is holder-only: a member browsing with a session must never burn the
    token someone else's install line is carrying."""
    user_id = await add_account(db, "member@example.com")
    other_id = await add_account(db, "someone.else@example.com")
    now = utc_now()
    raw = await download_tokens.mint(db, user_id, now)

    await download_tokens.spend(db, raw, other_id)

    assert await download_tokens.holder(db, raw, now) == user_id


@pytest.mark.asyncio
async def test_spending_nothing_is_a_no_op(db):
    """A member session downloads with no token in the URL at all."""
    user_id = await add_account(db, "member@example.com")
    now = utc_now()
    raw = await download_tokens.mint(db, user_id, now)

    await download_tokens.spend(db, None, user_id)
    await download_tokens.spend(db, "not-a-token", user_id)
    await download_tokens.spend(db, "dl_" + "x" * 32, user_id)

    assert await download_tokens.holder(db, raw, now) == user_id


@pytest.mark.asyncio
async def test_a_gateway_token_is_not_a_download_token(db):
    """Purpose is a namespace: the relay handshake token opens no download."""
    user_id = await add_account(db, "member@example.com")
    now = utc_now()
    raw = await ephemeral_tokens.mint(
        db,
        purpose="gateway",
        prefix="dl_",
        user_id=user_id,
        ttl=timedelta(minutes=5),
        now=now,
    )
    assert await download_tokens.holder(db, raw, now) is None


@pytest.mark.asyncio
async def test_minting_purges_the_tokens_that_have_already_expired(db):
    user_id = await add_account(db, "member@example.com")
    now = utc_now()
    db.add(
        EphemeralToken(
            token_hash=token_hash("dl_stale"),
            purpose=download_tokens.PURPOSE,
            user_id=user_id,
            expires_at=now - timedelta(seconds=1),
        )
    )
    await db.commit()

    raw = await download_tokens.mint(db, user_id, now)

    assert [row.token_hash for row in await _rows(db)] == [token_hash(raw)]
