"""Tests pinning the transport_sessions migration to the model it materialises:
the revision chain, the column order, and the named constraints."""

from __future__ import annotations

import importlib.util
from pathlib import Path
from unittest.mock import patch

import sqlalchemy as sa

from buildapp.models import TransportSession

_MIGRATION_PATH = (
    Path(__file__).resolve().parent.parent
    / "migrations"
    / "versions"
    / "20260905_170000_20bec806d0e5_transport_sessions.py"
)


def _load_migration():
    spec = importlib.util.spec_from_file_location(
        "transport_sessions_migration", _MIGRATION_PATH
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _upgrade_calls():
    migration = _load_migration()
    with patch.object(migration.op, "create_table") as create_table, patch.object(
        migration.op, "create_index"
    ) as create_index, patch.object(migration.op, "f", side_effect=lambda name: name):
        migration.upgrade()
    create_table.assert_called_once()
    return create_table.call_args, create_index.call_args_list


def test_revision_chains_from_the_desktop_oauth_client():
    migration = _load_migration()
    assert migration.revision == "20bec806d0e5"
    assert migration.down_revision == "c9e3a5b7d8f0"


def test_upgrade_creates_transport_sessions_with_the_model_columns_in_order():
    (table_name, *elements), _ = _upgrade_calls()[0]
    assert table_name == "transport_sessions"
    column_names = [e.name for e in elements if isinstance(e, sa.Column)]
    assert column_names == list(TransportSession.__table__.columns.keys())


def test_upgrade_names_the_primary_key_and_the_device_session_uniqueness():
    (_, *elements), _ = _upgrade_calls()[0]
    constraint_names = {e.name for e in elements if isinstance(e, sa.Constraint)}
    assert "pk_transport_sessions" in constraint_names
    assert "uq_transport_sessions_device_session" in constraint_names


def test_upgrade_indexes_what_the_admin_page_filters_and_groups_by():
    _, index_calls = _upgrade_calls()
    indexed = {tuple(call.args[2]) for call in index_calls}
    assert indexed == {("session_id",), ("device_id",), ("owner_user_id",), ("minted_at",)}
    model_indexed = {
        (column.name,) for column in TransportSession.__table__.columns if column.index
    }
    assert indexed == model_indexed


def test_downgrade_drops_the_indexes_and_the_table():
    migration = _load_migration()
    with patch.object(migration.op, "drop_table") as drop_table, patch.object(
        migration.op, "drop_index"
    ) as drop_index, patch.object(migration.op, "f", side_effect=lambda name: name):
        migration.downgrade()
    drop_table.assert_called_once_with("transport_sessions")
    assert drop_index.call_count == 4
