"""Process configuration, migration, election and commit-time wakeups."""

import asyncio
import os
from uuid import uuid4

import pytest
from alembic.migration import MigrationContext
from alembic.operations import Operations
from sqlalchemy import create_engine, inspect, text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from buildapp.db_test_support import create_skrift_tables, session_maker_for
from buildapp.hosted_tunnel_assignments import notify_hosted_tunnels_changed
from buildapp.hosted_tunnel_provisioner import postgres_reconcile_lease
from buildapp.hosted_tunnel_worker import read_secret_file, wait_for_database_changes
from buildapp.migration_test_support import load_migration, migration_path
from buildapp.test_hosted_tunnel_provisioner import TUNNELS, hosted_environment

MIGRATION = migration_path("20261124_200000_c3d4e5f6a7b8_hosted_tunnels.py")


def apply_migration(connection, *, downgrade=False):
    migration = load_migration(MIGRATION)
    assert migration.down_revision == "b2c3d4e5f6a7"
    with Operations.context(MigrationContext.configure(connection)):
        (migration.downgrade if downgrade else migration.upgrade)()


def test_migration_is_one_reversible_step():
    with create_engine("sqlite://").begin() as connection:
        apply_migration(connection)
        schema = inspect(connection)
        assert schema.get_pk_constraint("hosted_tunnel_assignments")["constrained_columns"] == ["device_id"]
        assert schema.get_foreign_keys("hosted_tunnel_assignments") == []
        apply_migration(connection, downgrade=True)
        assert inspect(connection).get_table_names() == []


def test_mount_token_is_bounded_and_safe_to_report(tmp_path, monkeypatch):
    mount = tmp_path / "synthetic-token"
    monkeypatch.setenv("TEST_TOKEN_FILE", str(mount))
    mount.write_text("fixture-token\n")
    assert read_secret_file("TEST_TOKEN_FILE") == "fixture-token"
    mount.write_text("first\nsecond\n")
    with pytest.raises(ValueError, match="TEST_TOKEN_FILE") as failure:
        read_secret_file("TEST_TOKEN_FILE")
    assert "first" not in str(failure.value)


@pytest.mark.asyncio
async def test_postgres_election_migration_and_notifications():
    raw_url = os.environ.get("HOSTED_TUNNEL_TEST_POSTGRES_URL")
    if not raw_url:
        pytest.skip("requires disposable loopback PostgreSQL")
    url = make_url(raw_url)
    assert url.drivername == "postgresql+asyncpg" and not url.query
    assert url.host in {"localhost", "127.0.0.1", "::1"}
    assert url.database == url.username == "hosted_tunnel_test"
    random_database = "hosted_tunnel_test_" + uuid4().hex
    admin = create_async_engine(url, isolation_level="AUTOCOMMIT")
    engine = create_async_engine(url.set(database=random_database))
    try:
        async with admin.connect() as connection:
            await connection.execute(text(f"CREATE DATABASE {random_database}"))
        await prove_postgres(engine)
    finally:
        await engine.dispose()
        async with admin.connect() as connection:
            await connection.execute(text(f"DROP DATABASE IF EXISTS {random_database} WITH (FORCE)"))
        await admin.dispose()


async def prove_postgres(engine):
    await create_skrift_tables(engine)
    async with engine.begin() as connection:
        await connection.execute(text("DROP TABLE hosted_tunnel_assignments"))
        await connection.run_sync(apply_migration)

    changed = asyncio.Event()
    async with asyncio.timeout(4):
        async with wait_for_database_changes(engine, changed.set):
            await prove_notification_boundaries(engine, changed)
            async with postgres_reconcile_lease(engine, "bridges.example.test") as first:
                assert first is not None
                await first()
                async with postgres_reconcile_lease(engine, "bridges.example.test") as competitor:
                    assert competitor is None
                async with postgres_reconcile_lease(engine, "other.example.test") as other:
                    assert other is not None
            async with postgres_reconcile_lease(engine, "bridges.example.test") as successor:
                assert successor is not None

    await prove_elected_worker(engine)
    async with engine.begin() as connection:
        await connection.run_sync(apply_migration, downgrade=True)


async def prove_notification_boundaries(engine, changed):
    sessions = async_sessionmaker(engine)
    async with sessions() as db:
        await notify_hosted_tunnels_changed(db)
        assert not changed.is_set()
        await db.rollback()
    async with sessions.begin() as db:
        await db.execute(text("UPDATE account_policies SET tunnel_access = false WHERE false"))
        assert not changed.is_set()
    await changed.wait()
    changed.clear()
    async with sessions.begin() as db:
        await notify_hosted_tunnels_changed(db)
        assert not changed.is_set()
    await changed.wait()


async def prove_elected_worker(engine):
    async with hosted_environment(session_maker_for(engine)) as env:
        reached_provider = asyncio.Event()
        release_provider = asyncio.Event()

        async def before(method, path):
            if (method, path) == ("GET", TUNNELS):
                reached_provider.set()
                await release_provider.wait()

        env.api.before = before
        first_pass = asyncio.create_task(env.worker(lease=None).reconcile_once())
        try:
            async with asyncio.timeout(5):
                await reached_provider.wait()
                assert (await env.row()).state == "pending"
                await env.worker(lease=None).reconcile_once()
                assert env.api.history == [("GET", TUNNELS)]
        finally:
            release_provider.set()
            await first_pass
        assert (await env.claimable()).state == "ready"
        assert len(env.api.tunnels) == len(env.api.dns) == 1
