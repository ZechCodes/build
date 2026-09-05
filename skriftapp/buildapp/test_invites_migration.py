"""Tests pinning the invites migration to the model it materialises: the revision
chain, the column order, the named constraints, and the indexes every invite lookup
(by token, by address, by inviter, by redeemer) reads."""

from __future__ import annotations

import importlib.util
from pathlib import Path
from unittest.mock import patch

import sqlalchemy as sa

from buildapp.models import Invite

_MIGRATION_PATH = (
    Path(__file__).resolve().parent.parent
    / "migrations"
    / "versions"
    / "20260906_090000_d1f2a3b4c5e6_invites.py"
)


def _load_migration():
    spec = importlib.util.spec_from_file_location("invites_migration", _MIGRATION_PATH)
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


def test_revision_chains_from_the_transport_sessions_migration():
    migration = _load_migration()
    assert migration.revision == "d1f2a3b4c5e6"
    assert migration.down_revision == "20bec806d0e5"


def test_upgrade_creates_invites_with_the_model_columns_in_order():
    (table_name, *elements), _ = _upgrade_calls()[0]
    assert table_name == "invites"
    column_names = [e.name for e in elements if isinstance(e, sa.Column)]
    assert column_names == list(Invite.__table__.columns.keys())


def test_upgrade_names_the_primary_key_the_token_uniqueness_and_both_user_keys():
    (_, *elements), _ = _upgrade_calls()[0]
    constraint_names = {e.name for e in elements if isinstance(e, sa.Constraint)}
    assert "pk_invites" in constraint_names
    assert "uq_invites_token_hash" in constraint_names
    assert "fk_invites_invited_by_users" in constraint_names
    assert "fk_invites_redeemed_by_users" in constraint_names


def test_upgrade_indexes_every_column_an_invite_is_looked_up_by():
    _, index_calls = _upgrade_calls()
    indexed = {tuple(call.args[2]) for call in index_calls}
    assert indexed == {
        ("token_hash",),
        ("email",),
        ("invited_by",),
        ("redeemed_by",),
    }
    model_indexed = {
        (column.name,) for column in Invite.__table__.columns if column.index
    }
    assert indexed == model_indexed


def test_downgrade_drops_the_indexes_and_the_table():
    migration = _load_migration()
    with patch.object(migration.op, "drop_table") as drop_table, patch.object(
        migration.op, "drop_index"
    ) as drop_index, patch.object(migration.op, "f", side_effect=lambda name: name):
        migration.downgrade()
    drop_table.assert_called_once_with("invites")
    assert drop_index.call_count == 4
