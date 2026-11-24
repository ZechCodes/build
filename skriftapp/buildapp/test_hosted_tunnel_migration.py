"""The hosted-mode revision preserves registrations from the preceding head."""

from uuid import uuid4

import pytest
import sqlalchemy as sa
from advanced_alchemy.types import GUID
from alembic.migration import MigrationContext
from alembic.operations import Operations
from skrift.db.base import Base

from buildapp.migration_test_support import load_migration, migration_path
from buildapp.models import HostedModeReport


@pytest.mark.parametrize("target_metadata", [None, Base.metadata], ids=["standalone", "skrift"])
def test_hosted_mode_upgrade_and_downgrade(target_metadata):
    authority = load_migration(migration_path("20261029_220000_a910b2c3d4e5_bridge_http_authority.py"))
    policy = load_migration(migration_path("20261124_190000_b2c3d4e5f6a7_account_policy.py"))
    hosted = load_migration(migration_path("20261124_203000_4ac3e7d238b1_hosted_mode_report.py"))
    assert hosted.down_revision == policy.revision
    engine = sa.create_engine("sqlite://")
    account_id, device_id, endpoint_id = (uuid4().bytes for _ in range(3))
    with engine.begin() as db:
        db.exec_driver_sql("PRAGMA foreign_keys=ON")
        fixture = sa.MetaData()
        sa.Table("users", fixture, sa.Column("id", GUID(length=16), primary_key=True))
        sa.Table(
            "devices", fixture, sa.Column("id", GUID(length=16), primary_key=True),
            sa.Column("approved", sa.Boolean(), nullable=False),
        )
        fixture.create_all(db)
        db.exec_driver_sql("INSERT INTO users VALUES (?)", (account_id,))
        db.exec_driver_sql("INSERT INTO devices VALUES (?, 1)", (device_id,))
        with Operations.context(MigrationContext.configure(db, opts={"target_metadata": target_metadata})):
            authority.upgrade()
            policy.upgrade()
            db.exec_driver_sql(
                "INSERT INTO bridge_http_endpoints "
                "(id, device_id, user_id, origin, identity_fingerprint, endpoint_epoch, "
                "authorization_epoch, enabled, created_at, updated_at) "
                "VALUES (?, ?, ?, 'https://personal.example', 'fixture', 'epoch', 0, 1, "
                "CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                (endpoint_id, device_id, account_id),
            )
            hosted.upgrade()
            inspector = sa.inspect(db)
            assert {
                c["name"] for c in inspector.get_columns("hosted_mode_reports")
            } == set(HostedModeReport.__table__.columns.keys())
            assert db.exec_driver_sql(
                "SELECT origin, enabled, assignment_generation FROM bridge_http_endpoints"
            ).one() == ("https://personal.example", 1, None)
            hosted.downgrade()
        assert "hosted_mode_reports" not in sa.inspect(db).get_table_names()
        assert db.exec_driver_sql(
            "SELECT origin, enabled FROM bridge_http_endpoints"
        ).one() == ("https://personal.example", 1)
    engine.dispose()
