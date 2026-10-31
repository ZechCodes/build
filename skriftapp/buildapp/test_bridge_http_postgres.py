"""Opt-in PostgreSQL concurrency witnesses using separate physical connections.

BRIDGE_HTTP_TEST_POSTGRES_URL must name the loopback-only disposable database
and role ``bridge_http_test``. Each case creates and drops its own database.
No production or existing application schema is modified. The normal unit gate
skips these cases when the explicitly guarded test URL is absent.
"""

import asyncio
import json
import os
from uuid import UUID, uuid4

import pytest
import pytest_asyncio
from litestar.exceptions import ClientException, NotAuthorizedException, TooManyRequestsException
from sqlalchemy import DDL, func, select, text
from sqlalchemy.engine import make_url
from sqlalchemy.exc import DBAPIError

from buildapp import bridge_http_auth as auth
from buildapp.bridge_http_models import (
    BridgeHttpChallenge,
    BridgeHttpClient,
    BridgeHttpEndpoint,
    BridgeHttpFamily,
    BridgeHttpReplay,
    BridgeHttpToken,
)
from buildapp.clock import utc_now
from buildapp.db_test_support import (
    create_skrift_tables,
    postgres_proof_engine,
    session_maker_for,
)
from buildapp.test_bridge_http_auth import ISSUER, ORIGIN, registered, signed


def witness(event, **observations):
    """Emit only server/connection observations, never credential material."""
    print(
        "postgres-witness "
        + json.dumps({"event": event, **observations}, sort_keys=True),
        flush=True,
    )


@pytest_asyncio.fixture
async def postgres_sessions(request):
    raw_url = os.environ.get("BRIDGE_HTTP_TEST_POSTGRES_URL")
    if not raw_url:
        pytest.skip("requires explicitly configured disposable local PostgreSQL")
    url = make_url(raw_url)
    assert url.drivername == "postgresql+asyncpg" and not url.query
    assert url.host in {"localhost", "127.0.0.1", "::1"}
    assert url.database == url.username == "bridge_http_test"
    database = "bridge_http_test_" + uuid4().hex
    admin = postgres_proof_engine(raw_url)
    engine = postgres_proof_engine(url.set(database=database))
    try:
        async with admin.connect() as connection:
            connection = await connection.execution_options(
                isolation_level="AUTOCOMMIT"
            )
            version_number = int(
                await connection.scalar(text("SHOW server_version_num"))
            )
            assert version_number >= 160000
            witness(
                "server",
                test=request.node.nodeid,
                version=await connection.scalar(text("SELECT version()")),
                version_number=version_number,
            )
            await connection.execute(
                DDL(
                    "CREATE DATABASE %(database)s",
                    context={
                        "database": connection.dialect.identifier_preparer.quote_identifier(
                            database
                        )
                    },
                )
            )
        await create_skrift_tables(engine)
        yield session_maker_for(
            engine,
            info={"postgres_witness_test": request.node.nodeid},
        )
    finally:
        await engine.dispose()
        async with admin.connect() as connection:
            connection = await connection.execution_options(
                isolation_level="AUTOCOMMIT"
            )
            await connection.execute(
                DDL(
                    "DROP DATABASE IF EXISTS %(database)s WITH (FORCE)",
                    context={
                        "database": connection.dialect.identifier_preparer.quote_identifier(
                            database
                        )
                    },
                )
            )
        await admin.dispose()


async def owner_with_login(maker):
    async with maker() as db:
        user, device, key = await registered(db)
        binding = "browser:" + uuid4().hex
        db.add(BridgeHttpClient(user_id=user, binding_hash=auth.hash_token(binding)))
        await db.commit()
    return user, device, key, binding


async def pending_proof(db, user, device, key, binding):
    pending = await auth.create_discovery(db, user, binding, ORIGIN, ISSUER, utc_now())
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
    return pending, proof


async def complete(db, user, binding, pending, proof):
    return await auth.complete_discovery(
        db, user, binding, pending["challenge_id"], proof, ISSUER, utc_now()
    )


async def assert_blocked(maker, contender, holder):
    """Observe the server's lock graph, not elapsed time or task scheduling."""
    assert contender != holder

    async def observe():
        async with maker() as db:
            observer = await db.scalar(text("SELECT pg_backend_pid()"))
            assert observer not in (holder, contender)
            while True:
                blockers = await db.scalar(
                    text("SELECT pg_blocking_pids(:contender)"),
                    {"contender": contender},
                )
                if holder in blockers:
                    witness(
                        "blocking",
                        test=maker.kw["info"]["postgres_witness_test"],
                        holder_pid=holder,
                        contender_pid=contender,
                        observer_pid=observer,
                        pg_blocking_pids=sorted(blockers),
                    )
                    return
                await asyncio.sleep(0.005)

    await asyncio.wait_for(observe(), timeout=5)


async def assert_inactive(db, device, key, token):
    assertion = signed(key, device, "introspection", token_hash=auth.hash_token(token))
    assert await auth.introspect(db, token, assertion, ISSUER, utc_now()) == {
        "active": False
    }


@pytest.mark.asyncio
async def test_postgres_conditional_spend_refuses_a_stale_independent_reader(
    postgres_sessions,
):
    maker = postgres_sessions
    user, device, key, binding = await owner_with_login(maker)
    async with maker() as db:
        pending, _ = await pending_proof(db, user, device, key, binding)
        await db.commit()
    async with maker() as first, maker() as second:
        first_pid = await first.scalar(text("SELECT pg_backend_pid()"))
        second_pid = await second.scalar(text("SELECT pg_backend_pid()"))
        challenge_id = UUID(pending["challenge_id"])
        first_read = await first.get(BridgeHttpChallenge, challenge_id)
        stale_read = await second.get(BridgeHttpChallenge, challenge_id)
        assert first_read.consumed_at is stale_read.consumed_at is None
        await auth._spend(first, first_read, pending["nonce"], utc_now())
        competing = asyncio.create_task(
            auth._spend(second, stale_read, pending["nonce"], utc_now())
        )
        try:
            await assert_blocked(maker, second_pid, first_pid)
            await first.commit()
            with pytest.raises(NotAuthorizedException):
                await asyncio.wait_for(competing, timeout=5)
        finally:
            competing.cancel()
            await asyncio.gather(competing, return_exceptions=True)
            await first.rollback()
            await second.rollback()
    async with maker() as db:
        assert (await db.get(BridgeHttpChallenge, challenge_id)).consumed_at is not None


@pytest.mark.asyncio
async def test_postgres_unique_jti_survives_competing_insert_transactions(
    postgres_sessions,
):
    maker = postgres_sessions
    _, device, key, _ = await owner_with_login(maker)
    now = utc_now()
    proof = signed(
        key, device, "introspection", token_hash=auth.hash_token("bhttp_test")
    )
    claims = auth.verify(proof, device, "introspection", ISSUER, now)
    async with maker() as first, maker() as second:
        first_pid = await first.scalar(text("SELECT pg_backend_pid()"))
        second_pid = await second.scalar(text("SELECT pg_backend_pid()"))
        await auth._record_replay(first, device, claims, now)
        competing = asyncio.create_task(
            auth._record_replay(second, device, claims, now)
        )
        try:
            await assert_blocked(maker, second_pid, first_pid)
            await first.commit()
            with pytest.raises(NotAuthorizedException):
                await asyncio.wait_for(competing, timeout=5)
        finally:
            competing.cancel()
            await asyncio.gather(competing, return_exceptions=True)
            await first.rollback()
            await second.rollback()
    async with maker() as db:
        assert (
            await db.scalar(
                select(func.count())
                .select_from(BridgeHttpReplay)
                .where(
                    BridgeHttpReplay.device_id == device.id,
                    BridgeHttpReplay.jti == claims["jti"],
                )
            )
            == 1
        )


@pytest.mark.asyncio
async def test_postgres_competing_discovery_workers_mint_only_one_family(
    postgres_sessions,
):
    maker = postgres_sessions
    user, device, key, binding = await owner_with_login(maker)
    async with maker() as db:
        pending, proof = await pending_proof(db, user, device, key, binding)
        await db.commit()
    async with maker() as first, maker() as second:
        first_pid = await first.scalar(text("SELECT pg_backend_pid()"))
        second_pid = await second.scalar(text("SELECT pg_backend_pid()"))
        minted = await complete(first, user, binding, pending, proof)
        competing = asyncio.create_task(complete(second, user, binding, pending, proof))
        try:
            await assert_blocked(maker, second_pid, first_pid)
            await first.commit()
            with pytest.raises(NotAuthorizedException):
                await asyncio.wait_for(competing, timeout=5)
        finally:
            competing.cancel()
            await asyncio.gather(competing, return_exceptions=True)
            await first.rollback()
            await second.rollback()
    async with maker() as db:
        assert await db.scalar(select(func.count()).select_from(BridgeHttpFamily)) == 1
        assert await db.scalar(select(func.count()).select_from(BridgeHttpToken)) == 1
        assertion = signed(
            key,
            device,
            "introspection",
            token_hash=auth.hash_token(minted["access_token"]),
        )
        assert (
            await auth.introspect(
                db, minted["access_token"], assertion, ISSUER, utc_now()
            )
        )["active"]


@pytest.mark.asyncio
async def test_postgres_endpoint_predecessor_cas_rejects_the_losing_registration(
    postgres_sessions,
):
    maker = postgres_sessions
    user, device, key, binding = await owner_with_login(maker)
    async with maker() as db:
        pending, proof = await pending_proof(db, user, device, key, binding)
        minted = await complete(db, user, binding, pending, proof)
        proofs = []
        for epoch in ("epoch-two", "epoch-three"):
            challenge = await auth.registration_challenge(
                db,
                device.id,
                signed(key, device, "registration-challenge"),
                ISSUER,
                utc_now(),
            )
            proofs.append(
                signed(
                    key,
                    device,
                    "registration",
                    challenge_id=challenge["challenge_id"],
                    nonce=challenge["nonce"],
                    origin=ORIGIN,
                    endpoint_epoch=epoch,
                    previous_endpoint_epoch="epoch-one",
                    enabled=True,
                )
            )
        await db.commit()
    async with maker() as first, maker() as second:
        first_pid = await first.scalar(text("SELECT pg_backend_pid()"))
        second_pid = await second.scalar(text("SELECT pg_backend_pid()"))
        await auth.register_endpoint(first, device.id, proofs[0], ISSUER, utc_now())
        competing = asyncio.create_task(
            auth.register_endpoint(second, device.id, proofs[1], ISSUER, utc_now())
        )
        try:
            await assert_blocked(maker, second_pid, first_pid)
            await first.commit()
            with pytest.raises(ClientException) as conflict:
                await asyncio.wait_for(competing, timeout=5)
            assert conflict.value.status_code == 409
        finally:
            competing.cancel()
            await asyncio.gather(competing, return_exceptions=True)
            await first.rollback()
            await second.rollback()
    async with maker() as db:
        endpoint = await db.scalar(
            select(BridgeHttpEndpoint).where(BridgeHttpEndpoint.device_id == device.id)
        )
        assert endpoint.endpoint_epoch == "epoch-two"
        family = await db.get(BridgeHttpFamily, UUID(minted["family_id"]))
        assert family.revoked_at is not None
        await assert_inactive(db, device, key, minted["access_token"])


@pytest.mark.asyncio
@pytest.mark.parametrize("issuance_kind", ["discovery", "renewal"])
@pytest.mark.parametrize("first_operation", ["issuance", "logout"])
async def test_postgres_logout_serializes_with_issuance_and_revokes_every_grant(
    postgres_sessions, issuance_kind, first_operation
):
    maker = postgres_sessions
    user, device, key, binding = await owner_with_login(maker)
    old_token = None
    async with maker() as db:
        pending, proof = await pending_proof(db, user, device, key, binding)
        if issuance_kind == "renewal":
            initial = await complete(db, user, binding, pending, proof)
            old_token = initial["access_token"]
        await db.commit()

    async def issue(db):
        if issuance_kind == "discovery":
            return await complete(db, user, binding, pending, proof)
        return await auth.renew(
            db,
            user,
            binding,
            device.id,
            initial["family_id"],
            "epoch-one",
            ISSUER,
            utc_now(),
        )

    async def logout(db):
        await auth.revoke_client(db, user, binding, utc_now())

    actions = {"issuance": issue, "logout": logout}
    second_operation = "logout" if first_operation == "issuance" else "issuance"
    minted = None
    async with maker() as first, maker() as second:
        first_pid = await first.scalar(text("SELECT pg_backend_pid()"))
        second_pid = await second.scalar(text("SELECT pg_backend_pid()"))
        first_result = await actions[first_operation](first)
        competing = asyncio.create_task(actions[second_operation](second))
        try:
            await assert_blocked(maker, second_pid, first_pid)
            await first.commit()
            if first_operation == "logout":
                with pytest.raises(NotAuthorizedException):
                    await asyncio.wait_for(competing, timeout=5)
                await second.rollback()
            else:
                minted = first_result
                await asyncio.wait_for(competing, timeout=5)
                await second.commit()
        finally:
            competing.cancel()
            await asyncio.gather(competing, return_exceptions=True)
            await first.rollback()
            await second.rollback()
    async with maker() as db:
        client = await db.scalar(
            select(BridgeHttpClient).where(
                BridgeHttpClient.user_id == user,
                BridgeHttpClient.binding_hash == auth.hash_token(binding),
            )
        )
        assert client.revoked_at is not None
        families = (await db.scalars(select(BridgeHttpFamily))).all()
        assert all(family.revoked_at is not None for family in families)
        assert len(families) == (
            0 if issuance_kind == "discovery" and first_operation == "logout" else 1
        )
        for token in [
            token
            for token in (old_token, minted["access_token"] if minted else None)
            if token
        ]:
            await assert_inactive(db, device, key, token)
        with pytest.raises(NotAuthorizedException):
            await auth.create_discovery(db, user, binding, ORIGIN, ISSUER, utc_now())


async def allocate(db, device, proof):
    return await auth.registration_challenge(db, device.id, proof, ISSUER, utc_now())


async def pending_registration_count(db, device):
    return await db.scalar(
        select(func.count())
        .select_from(BridgeHttpChallenge)
        .where(
            BridgeHttpChallenge.device_id == device.id,
            BridgeHttpChallenge.purpose == "registration",
            BridgeHttpChallenge.consumed_at.is_(None),
        )
    )


async def proof_replay_count(db, device, proof):
    claims = auth.verify(proof, device, "registration-challenge", ISSUER, utc_now())
    return await db.scalar(
        select(func.count())
        .select_from(BridgeHttpReplay)
        .where(
            BridgeHttpReplay.device_id == device.id,
            BridgeHttpReplay.jti == claims["jti"],
        )
    )


@pytest.mark.asyncio
async def test_postgres_duplicate_allocation_proof_mints_exactly_one_nonce(
    postgres_sessions,
):
    maker = postgres_sessions
    _, device, key, _ = await owner_with_login(maker)
    proof = signed(key, device, "registration-challenge")
    async with maker() as first, maker() as second:
        first_pid = await first.scalar(text("SELECT pg_backend_pid()"))
        second_pid = await second.scalar(text("SELECT pg_backend_pid()"))
        allocated = await allocate(first, device, proof)
        competing = asyncio.create_task(allocate(second, device, proof))
        try:
            await assert_blocked(maker, second_pid, first_pid)
            await first.commit()
            with pytest.raises(NotAuthorizedException):
                await asyncio.wait_for(competing, timeout=5)
            # Even a caller committing after catching the refusal cannot allocate.
            await second.commit()
        finally:
            competing.cancel()
            await asyncio.gather(competing, return_exceptions=True)
            await first.rollback()
            await second.rollback()
    async with maker() as db:
        assert await pending_registration_count(db, device) == 1
        assert await proof_replay_count(db, device, proof) == 1
        assert await db.get(BridgeHttpChallenge, UUID(allocated["challenge_id"]))


@pytest.mark.asyncio
async def test_postgres_distinct_allocation_proofs_cannot_race_past_eight_pending(
    postgres_sessions,
):
    maker = postgres_sessions
    _, device, key, _ = await owner_with_login(maker)
    pending = []
    async with maker() as db:
        for _ in range(7):
            pending.append(
                await allocate(
                    db, device, signed(key, device, "registration-challenge")
                )
            )
        await db.commit()
    accepted_proof = signed(key, device, "registration-challenge")
    refused_proof = signed(key, device, "registration-challenge")
    async with maker() as first, maker() as second:
        first_pid = await first.scalar(text("SELECT pg_backend_pid()"))
        second_pid = await second.scalar(text("SELECT pg_backend_pid()"))
        await allocate(first, device, accepted_proof)
        competing = asyncio.create_task(allocate(second, device, refused_proof))
        try:
            await assert_blocked(maker, second_pid, first_pid)
            await first.commit()
            with pytest.raises(TooManyRequestsException):
                await asyncio.wait_for(competing, timeout=5)
            await second.commit()
        finally:
            competing.cancel()
            await asyncio.gather(competing, return_exceptions=True)
            await first.rollback()
            await second.rollback()
    async with maker() as db:
        assert await pending_registration_count(db, device) == 8
        assert await proof_replay_count(db, device, accepted_proof) == 1
        assert await proof_replay_count(db, device, refused_proof) == 0
        spent = await db.get(BridgeHttpChallenge, UUID(pending[0]["challenge_id"]))
        await auth._spend(db, spent, pending[0]["nonce"], utc_now())
        await db.commit()
        # Capacity refusal does not burn a valid proof; its later retry can spend.
        await allocate(db, device, refused_proof)
        await db.commit()
    async with maker() as db:
        assert await pending_registration_count(db, device) == 8
        assert await proof_replay_count(db, device, refused_proof) == 1


FAILURE_FUNCTION = DDL("""
CREATE FUNCTION fail_nonce_allocation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'injected bridge nonce allocation failure'; END;
$$
""")
IMMEDIATE_FAILURE = DDL("""
CREATE TRIGGER fail_nonce_write BEFORE INSERT ON bridge_http_challenges
FOR EACH ROW EXECUTE FUNCTION fail_nonce_allocation()
""")
COMMIT_FAILURE = DDL("""
CREATE CONSTRAINT TRIGGER fail_nonce_write AFTER INSERT ON bridge_http_challenges
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION fail_nonce_allocation()
""")


@pytest.mark.asyncio
@pytest.mark.parametrize("fail_at_commit", [False, True], ids=["write", "commit"])
async def test_postgres_nonce_and_replay_are_atomic_when_database_write_or_commit_fails(
    postgres_sessions, fail_at_commit
):
    maker = postgres_sessions
    _, device, key, _ = await owner_with_login(maker)
    proof = signed(key, device, "registration-challenge")
    async with maker() as db:
        await db.execute(FAILURE_FUNCTION)
        await db.execute(COMMIT_FAILURE if fail_at_commit else IMMEDIATE_FAILURE)
        await db.commit()
        if fail_at_commit:
            allocated = await allocate(db, device, proof)
            assert allocated["nonce"]
            with pytest.raises(DBAPIError):
                await db.commit()
            await db.rollback()
        else:
            with pytest.raises(DBAPIError):
                await allocate(db, device, proof)
            # The function's SAVEPOINT must preserve the caller's ability to commit.
            await db.commit()
    async with maker() as db:
        assert await pending_registration_count(db, device) == 0
        assert await proof_replay_count(db, device, proof) == 0
        await db.execute(DDL("DROP TRIGGER fail_nonce_write ON bridge_http_challenges"))
        await db.commit()
        await allocate(db, device, proof)
        await db.commit()
    async with maker() as db:
        assert await pending_registration_count(db, device) == 1
        assert await proof_replay_count(db, device, proof) == 1
