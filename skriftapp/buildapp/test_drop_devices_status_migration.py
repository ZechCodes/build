"""The devices.status drop, run for real against SQLite: the column goes, the rows
stay, and a downgrade restores it as the nullable migration left it."""

from __future__ import annotations

from collections.abc import Iterator

import pytest
import sqlalchemy as sa
from alembic.migration import MigrationContext
from alembic.operations import Operations

from buildapp.migration_test_support import load_migration, migration_path

_MIGRATION_PATH = migration_path("20260926_031000_198bbccf0ca3_drop_devices_status.py")

# The devices columns around the one this revision drops, as 7c34bee528bd left them.
_DEVICES = """
CREATE TABLE devices (
    id VARCHAR(32) PRIMARY KEY,
    owner_user_id VARCHAR(32),
    approved BOOLEAN NOT NULL,
    status VARCHAR(20),
    last_seen_at DATETIME
)
"""


@pytest.fixture
def connection() -> Iterator[sa.Connection]:
    engine = sa.create_engine("sqlite://")
    with engine.begin() as connection:
        connection.exec_driver_sql(_DEVICES)
        connection.exec_driver_sql(
            "INSERT INTO devices (id, owner_user_id, approved) VALUES"
            " ('owned', 'owner', 1), ('pending', NULL, 0)"
        )
        yield connection
    engine.dispose()


def _run(connection: sa.Connection, step: str) -> None:
    migration = load_migration(_MIGRATION_PATH)
    with Operations.context(MigrationContext.configure(connection)):
        getattr(migration, step)()


def _columns(connection: sa.Connection) -> dict[str, bool]:
    return {c["name"]: c["nullable"] for c in sa.inspect(connection).get_columns("devices")}


def test_revision_chains_from_the_nullable_migration():
    migration = load_migration(_MIGRATION_PATH)
    assert migration.revision == "198bbccf0ca3"
    assert migration.down_revision == "7c34bee528bd"


def test_upgrade_drops_status_and_keeps_every_row(connection):
    _run(connection, "upgrade")

    assert "status" not in _columns(connection)
    rows = connection.exec_driver_sql("SELECT id FROM devices ORDER BY id").scalars().all()
    assert rows == ["owned", "pending"]


def test_downgrade_restores_status_nullable(connection):
    _run(connection, "upgrade")
    _run(connection, "downgrade")

    assert _columns(connection)["status"] is True
