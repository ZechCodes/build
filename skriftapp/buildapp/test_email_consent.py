"""Product mail is sent only to accounts that explicitly opted in at signup."""

from __future__ import annotations

import asyncio

from buildapp.clock import utc_now
from buildapp.db_test_support import add_account, create_skrift_tables, engine_for, in_memory_session_maker
from buildapp.email_consent import (
    consent_by_user_id,
    consenting_product_email_addresses,
    record_signup_consent,
    revoke_consent_for_email,
)


def test_consent_queries_require_an_explicit_true_and_timestamp():
    async def work():
        maker = in_memory_session_maker()
        engine = engine_for(maker)
        try:
            await create_skrift_tables(engine)
            async with maker() as session:
                opted_in = await add_account(session, "yes@example.com")
                opted_out = await add_account(session, "no@example.com")
                missing = await add_account(session, "missing@example.com")
                record_signup_consent(session, opted_in, True, utc_now())
                record_signup_consent(session, opted_out, False, utc_now())
                await session.commit()
                states = await consent_by_user_id(session, (opted_in, opted_out, missing))
                recipients = await consenting_product_email_addresses(session)
                assert states == {opted_in: True, opted_out: False, missing: False}
                assert recipients == ["yes@example.com"]
        finally:
            await engine.dispose()

    asyncio.run(work())


def test_unsubscribe_revokes_member_consent_by_canonical_address():
    async def work():
        maker = in_memory_session_maker()
        engine = engine_for(maker)
        try:
            await create_skrift_tables(engine)
            async with maker() as session:
                user_id = await add_account(session, "Alice@Example.COM")
                record_signup_consent(session, user_id, True, utc_now())
                await session.commit()
                await revoke_consent_for_email(session, "alice@example.com")
                await session.commit()
                assert await consent_by_user_id(session, (user_id,)) == {user_id: False}
                assert await consenting_product_email_addresses(session) == []
        finally:
            await engine.dispose()

    asyncio.run(work())
