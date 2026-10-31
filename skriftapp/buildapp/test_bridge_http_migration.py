"""Execute the authority migration over a legacy schema, then reverse it."""

import sqlalchemy as sa
from advanced_alchemy.types import GUID
from alembic.migration import MigrationContext
from alembic.operations import Operations

from buildapp.migration_test_support import load_migration, migration_path
from buildapp import bridge_http_models as models

PATH = migration_path("20261029_220000_a910b2c3d4e5_bridge_http_authority.py")


def test_migration_adds_constraints_indexes_and_preserves_legacy_approval():
    migration = load_migration(PATH)
    assert migration.revision == "a910b2c3d4e5"
    assert migration.down_revision == "e2a4b6c8d0f1"
    assert "buildapp" not in PATH.read_text()
    engine = sa.create_engine("sqlite://")
    with engine.begin() as connection:
        connection.exec_driver_sql("PRAGMA foreign_keys=ON")
        metadata = sa.MetaData()
        sa.Table("users", metadata, sa.Column("id", GUID(length=16), primary_key=True))
        sa.Table(
            "devices",
            metadata,
            sa.Column("id", GUID(length=16), primary_key=True),
            sa.Column("approved", sa.Boolean, nullable=False),
        )
        metadata.create_all(connection)
        connection.exec_driver_sql("INSERT INTO devices VALUES (randomblob(16), 1)")
        with Operations.context(MigrationContext.configure(connection)):
            migration.upgrade()
        assert connection.exec_driver_sql(
            "SELECT approved,authorization_epoch FROM devices"
        ).one() == (1, 0)
        inspector = sa.inspect(connection)
        device_id_type = next(
            column["type"]
            for column in inspector.get_columns("devices")
            if column["name"] == "id"
        )
        for model in (
            models.BridgeHttpEndpoint,
            models.BridgeHttpChallenge,
            models.BridgeHttpFamily,
            models.BridgeHttpToken,
            models.BridgeHttpReplay,
            models.BridgeHttpClient,
        ):
            table = model.__table__
            assert {c["name"] for c in inspector.get_columns(table.name)} == set(
                table.columns.keys()
            )
            expected = {tuple(c.name for c in index.columns) for index in table.indexes}
            assert {
                tuple(i["column_names"]) for i in inspector.get_indexes(table.name)
            } == expected
            assert all(
                fk["options"]["ondelete"] == "CASCADE"
                for fk in inspector.get_foreign_keys(table.name)
            )
        for table_name in (
            "bridge_http_endpoints",
            "bridge_http_challenges",
            "bridge_http_families",
            "bridge_http_replays",
        ):
            migrated_type = next(
                column["type"]
                for column in inspector.get_columns(table_name)
                if column["name"] == "device_id"
            )
            assert migrated_type._type_affinity is device_id_type._type_affinity
        with Operations.context(MigrationContext.configure(connection)):
            migration.downgrade()
        assert set(sa.inspect(connection).get_table_names()) == {"users", "devices"}
        assert (
            connection.exec_driver_sql("SELECT approved FROM devices").scalar_one() == 1
        )
