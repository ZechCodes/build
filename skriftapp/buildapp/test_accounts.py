"""One home for "what address is this account?" — asked one id at a time by the invite
redemption and the SPA shell, and all at once by the admin invites page."""

from __future__ import annotations

from uuid import uuid4

import pytest

from buildapp.accounts import account_email, addresses_by_id
from buildapp.db_test_support import add_account

FIRST = "first@example.com"
SECOND = "second@example.com"


@pytest.mark.asyncio
async def test_an_account_is_read_back_by_its_address(db):
    user_id = await add_account(db, FIRST)
    assert await account_email(db, user_id) == FIRST


@pytest.mark.asyncio
async def test_an_account_that_is_gone_has_the_empty_address(db):
    assert await account_email(db, uuid4()) == ""


@pytest.mark.asyncio
async def test_every_address_comes_back_keyed_by_its_account(db):
    first = await add_account(db, FIRST)
    second = await add_account(db, SECOND)
    assert await addresses_by_id(db) == {first: FIRST, second: SECOND}


@pytest.mark.asyncio
async def test_an_account_with_no_address_is_left_out(db):
    with_address = await add_account(db, FIRST)
    await add_account(db, "")
    assert await addresses_by_id(db) == {with_address: FIRST}
