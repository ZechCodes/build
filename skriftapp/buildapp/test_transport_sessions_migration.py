"""Tests pinning the transport_sessions migration to the model it materialises:
the revision chain, the column order, and the named constraints."""

from __future__ import annotations

import sqlalchemy as sa

from buildapp.migration_test_support import (
    downgrade_calls,
    load_migration,
    migration_path,
    table_elements,
    upgrade_calls,
)
from buildapp.models import TransportSession

_MIGRATION_PATH = migration_path("20260905_170000_20bec806d0e5_transport_sessions.py")


def test_revision_chains_from_the_desktop_oauth_client():
    migration = load_migration(_MIGRATION_PATH)
    assert migration.revision == "20bec806d0e5"
    assert migration.down_revision == "c9e3a5b7d8f0"


def test_upgrade_creates_transport_sessions_with_the_model_columns_in_order():
    table_name, elements = table_elements(_MIGRATION_PATH)
    assert table_name == "transport_sessions"
    column_names = [e.name for e in elements if isinstance(e, sa.Column)]
    assert column_names == list(TransportSession.__table__.columns.keys())


def test_upgrade_names_the_primary_key_and_the_device_session_uniqueness():
    _, elements = table_elements(_MIGRATION_PATH)
    constraint_names = {e.name for e in elements if isinstance(e, sa.Constraint)}
    assert "pk_transport_sessions" in constraint_names
    assert "uq_transport_sessions_device_session" in constraint_names


def test_upgrade_indexes_what_the_admin_page_filters_and_groups_by():
    _, index_calls = upgrade_calls(_MIGRATION_PATH)
    indexed = {tuple(call.args[2]) for call in index_calls}
    assert indexed == {("session_id",), ("device_id",), ("owner_user_id",), ("minted_at",)}
    model_indexed = {
        (column.name,) for column in TransportSession.__table__.columns if column.index
    }
    assert indexed == model_indexed


def test_downgrade_drops_the_indexes_and_the_table():
    drop_table, drop_index = downgrade_calls(_MIGRATION_PATH)
    drop_table.assert_called_once_with("transport_sessions")
    assert drop_index.call_count == 4
