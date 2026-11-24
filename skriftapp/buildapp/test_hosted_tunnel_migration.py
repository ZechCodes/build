"""The hosted-mode revision preserves registrations from the preceding head."""

from pathlib import Path
from uuid import uuid4

import pytest
import skrift
import sqlalchemy as sa
from advanced_alchemy.types import GUID
from alembic.migration import MigrationContext
from alembic.operations import Operations
from alembic.script import ScriptDirectory
from skrift.db.base import Base

from buildapp.db_test_support import engine_for, in_memory_session_maker
from buildapp.migration_test_support import MIGRATIONS_DIR, load_migration, migration_path
from buildapp.models import HostedModeReport

PERSONAL_ORIGIN = "https://personal.example"
HOSTED_ORIGIN = "https://b-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.hosted.example"


def insert_registration(db, account_id, device_id, origin):
    db.exec_driver_sql("INSERT INTO devices (id, approved) VALUES (?, 1)", (device_id,))
    db.exec_driver_sql(
        "INSERT INTO bridge_http_endpoints "
        "(id, device_id, user_id, origin, identity_fingerprint, endpoint_epoch, "
        "authorization_epoch, enabled, created_at, updated_at) "
        "VALUES (?, ?, ?, ?, 'fixture', 'epoch', 0, 1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        (uuid4().bytes, device_id, account_id, origin),
    )
    db.exec_driver_sql(
        "INSERT INTO bridge_http_families (id, user_id, device_id, issuer, client_binding, "
        "origin, endpoint_epoch, authorization_epoch, scope, created_at, updated_at) "
        "VALUES (?, ?, ?, 'https://issuer.example', 'browser:test', ?, 'epoch', 0, "
        "'bridge:read', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        (uuid4().bytes, account_id, device_id, origin),
    )


@pytest.mark.parametrize("target_metadata", [None, Base.metadata], ids=["standalone", "skrift"])
@pytest.mark.asyncio
async def test_hosted_mode_upgrade_and_downgrade(target_metadata):
    engine = engine_for(in_memory_session_maker())
    try:
        async with engine.begin() as connection:
            await connection.run_sync(check_upgrade_and_downgrade, target_metadata)
    finally:
        await engine.dispose()


def test_hosted_mode_revision_is_the_single_head():
    skrift_versions = Path(skrift.__file__).parent / "alembic" / "versions"
    revisions = ScriptDirectory(
        str(MIGRATIONS_DIR.parent), version_locations=[str(MIGRATIONS_DIR), str(skrift_versions)],
    )
    assert len(revisions.get_heads()) == 1
    assert revisions.get_revision("4ac3e7d238b1").down_revision == "c3d4e5f6a7b8"


def check_upgrade_and_downgrade(db, target_metadata):
    authority = load_migration(migration_path("20261029_220000_a910b2c3d4e5_bridge_http_authority.py"))
    policy = load_migration(migration_path("20261124_190000_b2c3d4e5f6a7_account_policy.py"))
    assignments = load_migration(migration_path("20261124_200000_c3d4e5f6a7b8_hosted_tunnels.py"))
    hosted = load_migration(migration_path("20261124_203000_4ac3e7d238b1_hosted_mode_report.py"))
    assert hosted.down_revision == assignments.revision
    account_id, personal_id, hosted_id = (uuid4().bytes for _ in range(3))
    db.exec_driver_sql("PRAGMA foreign_keys=ON")
    fixture = sa.MetaData()
    sa.Table("users", fixture, sa.Column("id", GUID(length=16), primary_key=True))
    sa.Table(
        "devices", fixture, sa.Column("id", GUID(length=16), primary_key=True),
        sa.Column("approved", sa.Boolean(), nullable=False),
    )
    fixture.create_all(db)
    db.exec_driver_sql("INSERT INTO users VALUES (?)", (account_id,))
    with Operations.context(MigrationContext.configure(db, opts={"target_metadata": target_metadata})):
        authority.upgrade()
        policy.upgrade()
        assignments.upgrade()
        insert_registration(db, account_id, personal_id, PERSONAL_ORIGIN)
        hosted.upgrade()
        inspector = sa.inspect(db)
        generation_type = next(
            c["type"] for c in inspector.get_columns("bridge_http_endpoints")
            if c["name"] == "assignment_generation"
        )
        assert isinstance(generation_type, sa.BigInteger)
        assert {
            c["name"] for c in inspector.get_columns("hosted_mode_reports")
        } == set(HostedModeReport.__table__.columns.keys())
        assert db.exec_driver_sql(
            "SELECT origin, enabled, assignment_generation FROM bridge_http_endpoints"
        ).one() == (PERSONAL_ORIGIN, 1, None)
        insert_registration(db, account_id, hosted_id, HOSTED_ORIGIN)
        db.exec_driver_sql(
            "UPDATE bridge_http_endpoints SET assignment_generation = ? WHERE device_id = ?",
            (2**51 + 17, hosted_id),
        )
        hosted.downgrade()
    tables = sa.inspect(db).get_table_names()
    assert "hosted_mode_reports" not in tables
    assert "hosted_tunnel_assignments" in tables
    assert dict(db.exec_driver_sql(
        "SELECT origin, enabled FROM bridge_http_endpoints"
    ).all()) == {PERSONAL_ORIGIN: 1, HOSTED_ORIGIN: 0}
    assert dict(db.exec_driver_sql(
        "SELECT origin, revoked_at IS NOT NULL FROM bridge_http_families"
    ).all()) == {PERSONAL_ORIGIN: 0, HOSTED_ORIGIN: 1}
