"""The devices.status nullable migration, run for real against SQLite: the column
the app no longer maps must accept a row without it, every stale value goes, and a
downgrade refills what the old writers would have left."""

from __future__ import annotations

from collections.abc import Iterator

import pytest
import sqlalchemy as sa
from alembic.migration import MigrationContext
from alembic.operations import Operations

from buildapp.migration_test_support import load_migration, migration_path

_MIGRATION_PATH = migration_path("20260926_030000_7c34bee528bd_devices_status_nullable.py")

# The devices columns this revision touches or reads, as the table stands before it.
_DEVICES = """
CREATE TABLE devices (
    id VARCHAR(32) PRIMARY KEY,
    owner_user_id VARCHAR(32),
    approved BOOLEAN NOT NULL,
    status VARCHAR(20) NOT NULL,
    last_seen_at DATETIME
)
"""


@pytest.fixture
def connection() -> Iterator[sa.Connection]:
    engine = sa.create_engine("sqlite://")
    with engine.begin() as connection:
        connection.exec_driver_sql(_DEVICES)
        connection.exec_driver_sql(
            "INSERT INTO devices (id, owner_user_id, approved, status) VALUES"
            " ('stale', 'owner', 1, 'online'),"
            " ('pending', NULL, 0, 'pending'),"
            " ('revoked', 'owner', 0, 'offline')"
        )
        yield connection
    engine.dispose()


def _run(connection: sa.Connection, step: str) -> None:
    migration = load_migration(_MIGRATION_PATH)
    with Operations.context(MigrationContext.configure(connection)):
        getattr(migration, step)()


def _statuses(connection: sa.Connection) -> dict[str, str | None]:
    return dict(connection.exec_driver_sql("SELECT id, status FROM devices").all())


def _status_nullable(connection: sa.Connection) -> bool:
    (status,) = [c for c in sa.inspect(connection).get_columns("devices") if c["name"] == "status"]
    return status["nullable"]


def test_revision_chains_from_the_invites_migration():
    migration = load_migration(_MIGRATION_PATH)
    assert migration.revision == "7c34bee528bd"
    assert migration.down_revision == "d1f2a3b4c5e6"


def test_upgrade_makes_status_nullable_and_clears_every_row(connection):
    _run(connection, "upgrade")

    assert _status_nullable(connection)
    assert _statuses(connection) == {"stale": None, "pending": None, "revoked": None}
    connection.exec_driver_sql(
        "INSERT INTO devices (id, owner_user_id, approved) VALUES ('new', NULL, 0)"
    )


def test_downgrade_refills_pending_until_claimed_and_offline_after(connection):
    _run(connection, "upgrade")
    connection.exec_driver_sql(
        "INSERT INTO devices (id, owner_user_id, approved) VALUES ('new', NULL, 0)"
    )

    _run(connection, "downgrade")

    assert not _status_nullable(connection)
    assert _statuses(connection) == {
        "stale": "offline",
        "pending": "pending",
        "revoked": "offline",
        "new": "pending",
    }
