"""The wiring the tests share is shared: no test file builds its own engine or its own
session maker, and membership is made the way the application makes it rather than by
hand-writing the row that means it."""

from __future__ import annotations

from pathlib import Path
from uuid import uuid4

import pytest

from buildapp import db_test_support
from buildapp.alpha_membership import is_alpha_member
from buildapp.db_test_support import MEMBER_ADDRESS, add_member
from buildapp.invites import invite_state, InviteState

HERE = Path(__file__).resolve().parent
OWN_ENGINE = ("create_async_engine", "async_sessionmaker")


def _other_test_sources():
    return [
        path
        for path in sorted(HERE.glob("test_*.py"))
        if path.name != Path(__file__).name
    ]


def test_no_test_file_builds_its_own_database():
    for path in _other_test_sources():
        source = path.read_text()
        for spelling in OWN_ENGINE:
            assert spelling not in source, f"{path.name} builds its own {spelling}"


def test_the_only_engine_builder_is_this_module():
    source = Path(db_test_support.__file__).read_text()
    assert source.count("create_async_engine(") == 1


@pytest.mark.asyncio
async def test_add_member_makes_a_member_by_redeeming_a_real_invite(db):
    user_id = uuid4()
    invite = await add_member(db, user_id)
    assert invite.email == MEMBER_ADDRESS
    assert invite_state(invite, invite.redeemed_at) is InviteState.REDEEMED
    assert await is_alpha_member(db, user_id) is True
