"""Competing hosted workers may spend a discovery challenge only once."""

import asyncio

import pytest
from litestar.exceptions import NotAuthorizedException
from sqlalchemy import func, select

from buildapp import bridge_http_auth as auth
from buildapp.db_test_support import (
    create_skrift_tables,
    engine_for,
    file_session_maker,
)
from buildapp.models import BridgeHttpFamily
from buildapp.test_bridge_http_auth import ISSUER, ORIGIN, registered, signed
from buildapp.clock import utc_now


@pytest.mark.asyncio
async def test_competing_connections_cannot_mint_two_families(tmp_path):
    maker = file_session_maker(tmp_path / "authority.sqlite")
    engine = engine_for(maker)
    await create_skrift_tables(engine)
    try:
        async with maker() as db:
            user, device, key = await registered(db)
            pending = await auth.create_discovery(
                db, user, "browser:client", ORIGIN, ISSUER, utc_now()
            )
            proof = signed(
                key,
                device,
                "discovery",
                challenge_id=pending["challenge_id"],
                nonce=pending["nonce"],
                origin=ORIGIN,
                endpoint_epoch="epoch-one",
                issuer=ISSUER,
            )
            await db.commit()

        async def compete():
            async with maker() as db:
                try:
                    minted = await auth.complete_discovery(
                        db,
                        user,
                        "browser:client",
                        pending["challenge_id"],
                        proof,
                        ISSUER,
                        utc_now(),
                    )
                    await db.commit()
                    return minted
                except NotAuthorizedException:
                    await db.rollback()
                    return None

        results = await asyncio.gather(compete(), compete())
        assert sum(result is not None for result in results) == 1
        async with maker() as db:
            assert (
                await db.scalar(select(func.count()).select_from(BridgeHttpFamily)) == 1
            )
    finally:
        await engine.dispose()
