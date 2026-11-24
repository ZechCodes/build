"""Process configuration, migration, election and commit-time wakeups."""

import asyncio
import os
import sys
from argparse import Namespace
from types import ModuleType, SimpleNamespace

import pytest
from alembic.migration import MigrationContext
from alembic.operations import Operations
from sqlalchemy import false, inspect, update

from buildapp.db_test_support import (
    create_skrift_tables,
    disposable_postgres_database,
    engine_for,
    in_memory_session_maker,
    session_maker_for,
)
from buildapp.hosted_tunnel_assignments import notify_hosted_tunnels_changed
from buildapp.hosted_tunnel_models import HostedTunnelAssignment
from buildapp.hosted_tunnel_provisioner import postgres_reconcile_lease
from buildapp.hosted_tunnel_worker import _mode_source, _run, read_secret_file, wait_for_database_changes
from buildapp.migration_test_support import load_migration, migration_path
from buildapp.models import AccountPolicy
from buildapp.test_hosted_tunnel_provisioner import TUNNELS, hosted_environment

MIGRATION = migration_path("20261124_200000_c3d4e5f6a7b8_hosted_tunnels.py")
ISSUER_MODULE = "buildapp.hosted_tunnel_source"
MODE_SOURCE_SETTING = "HOSTED_TUNNEL_MODE_SOURCE"


def apply_migration(connection, *, downgrade=False):
    migration = load_migration(MIGRATION)
    assert migration.down_revision == "b2c3d4e5f6a7"
    with Operations.context(MigrationContext.configure(connection)):
        (migration.downgrade if downgrade else migration.upgrade)()


@pytest.mark.asyncio
async def test_migration_is_one_reversible_step():
    engine = engine_for(in_memory_session_maker())
    try:
        async with engine.begin() as connection:
            await connection.run_sync(prove_reversible_migration)
    finally:
        await engine.dispose()


def prove_reversible_migration(connection):
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


def test_mode_source_loads_the_signed_issuer_adapter(monkeypatch):
    adapter = ModuleType(ISSUER_MODULE)

    async def hosted_pairings(_db):
        return set()

    adapter.hosted_mode_source = SimpleNamespace(hosted_pairings=hosted_pairings)
    monkeypatch.setitem(sys.modules, ISSUER_MODULE, adapter)
    monkeypatch.setenv(MODE_SOURCE_SETTING, f"{ISSUER_MODULE}:hosted_mode_source")
    assert _mode_source() is adapter.hosted_mode_source


@pytest.mark.asyncio
@pytest.mark.parametrize("setting", ["fixture_unknown:hosted_mode_source", f"{ISSUER_MODULE}:other"])
async def test_worker_rejects_mode_sources_outside_the_allowlist(monkeypatch, setting):
    monkeypatch.setenv(MODE_SOURCE_SETTING, setting)
    monkeypatch.delenv("DATABASE_URL", raising=False)
    with pytest.raises(ValueError, match=MODE_SOURCE_SETTING):
        await _run(Namespace(command=None))


@pytest.mark.asyncio
async def test_worker_fails_closed_before_setup_without_the_issuer_adapter(monkeypatch):
    monkeypatch.setitem(sys.modules, ISSUER_MODULE, None)
    monkeypatch.setenv(MODE_SOURCE_SETTING, f"{ISSUER_MODULE}:hosted_mode_source")
    monkeypatch.delenv("DATABASE_URL", raising=False)
    with pytest.raises(ValueError, match="signed hosted-mode adapter"):
        await _run(Namespace(command=None))


@pytest.mark.asyncio
async def test_postgres_election_migration_and_notifications():
    raw_url = os.environ.get("HOSTED_TUNNEL_TEST_POSTGRES_URL")
    if not raw_url:
        pytest.skip("requires disposable loopback PostgreSQL")
    async with disposable_postgres_database(raw_url, role="hosted_tunnel_test") as engine:
        await prove_postgres(engine)


async def prove_postgres(engine):
    await create_skrift_tables(engine)
    async with engine.begin() as connection:
        await connection.run_sync(HostedTunnelAssignment.__table__.drop)
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
    sessions = session_maker_for(engine)
    async with sessions() as db:
        await notify_hosted_tunnels_changed(db)
        assert not changed.is_set()
        await db.rollback()
    async with sessions.begin() as db:
        await db.execute(update(AccountPolicy).where(false()).values(tunnel_access=False))
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
