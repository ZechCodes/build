"""Exercise the migration over pre-existing users and devices."""

from uuid import uuid4

import pytest
import sqlalchemy as sa
from advanced_alchemy.types import GUID
from alembic.migration import MigrationContext
from alembic.operations import Operations

from buildapp.migration_test_support import load_migration, migration_path


PATH = migration_path("20261124_190000_b2c3d4e5f6a7_account_policy.py")


def test_account_policy_migration_upgrade_constraints_and_downgrade():
    migration = load_migration(PATH)
    assert migration.down_revision == "a910b2c3d4e5"
    engine = sa.create_engine("sqlite://")
    old_user = uuid4()
    new_user = uuid4()
    with engine.begin() as db:
        db.exec_driver_sql("PRAGMA foreign_keys=ON")
        metadata = sa.MetaData()
        users = sa.Table("users", metadata, sa.Column("id", GUID(length=16), primary_key=True))
        devices = sa.Table(
            "devices", metadata,
            sa.Column("id", GUID(length=16), primary_key=True),
            sa.Column("approved", sa.Boolean, nullable=False),
        )
        metadata.create_all(db)
        db.execute(users.insert().values(id=old_user))
        db.execute(devices.insert().values(id=uuid4(), approved=True))
        with Operations.context(MigrationContext.configure(db)):
            migration.upgrade()
        db.execute(users.insert().values(id=new_user))
        assert db.exec_driver_sql("SELECT approved, hosted_ready_once FROM devices").one() == (1, 0)
        assert db.exec_driver_sql("SELECT count(*) FROM account_policies").scalar_one() == 0

        for user in (old_user, new_user):
            db.exec_driver_sql(
                "INSERT INTO account_policies (id, created_at, updated_at) "
                "VALUES (?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)", (user.bytes,),
            )
        assert db.exec_driver_sql(
            "SELECT account_type, tunnel_access, promotion_revision FROM account_policies"
        ).all() == [("free", 0, 0)] * 2

        with pytest.raises(sa.exc.IntegrityError):
            db.exec_driver_sql("UPDATE account_policies SET account_type = 'enterprise'")
        with pytest.raises(sa.exc.IntegrityError):
            db.exec_driver_sql(
                "INSERT INTO account_policies (id, created_at, updated_at) "
                "VALUES (randomblob(16), CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
            )

        db.execute(users.delete().where(users.c.id == new_user))
        assert db.exec_driver_sql("SELECT count(*) FROM account_policies").scalar_one() == 1
        with Operations.context(MigrationContext.configure(db)):
            migration.downgrade()
        assert set(sa.inspect(db).get_table_names()) == {"users", "devices"}
        assert db.exec_driver_sql("SELECT approved FROM devices").scalar_one() == 1
        assert db.execute(sa.select(users.c.id)).scalar_one() == old_user
    engine.dispose()
