"""Tests pinning the waitlist_signups migration to the model it materialises: the
revision chain, the column order, and the named primary-key and unique constraints."""

from __future__ import annotations

import importlib.util
from pathlib import Path
from unittest.mock import patch

import sqlalchemy as sa

from buildapp.models import WaitlistSignup
from buildapp.waitlist_address import MAX_WAITLIST_ADDRESS_LENGTH

_MIGRATION_PATH = (
    Path(__file__).resolve().parent.parent
    / "migrations"
    / "versions"
    / "20260901_120000_b8d2f4a6c7e9_waitlist_signups.py"
)


def _load_migration():
    spec = importlib.util.spec_from_file_location(
        "waitlist_signups_migration", _MIGRATION_PATH
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _create_table_call():
    migration = _load_migration()
    with patch.object(migration.op, "create_table") as create_table, patch.object(
        migration.op, "f", side_effect=lambda name: name
    ):
        migration.upgrade()
    create_table.assert_called_once()
    return create_table.call_args


def test_revision_chains_from_announced_app_versions():
    migration = _load_migration()
    assert migration.revision == "b8d2f4a6c7e9"
    assert migration.down_revision == "a7c1e2f3b4d5"


def test_upgrade_creates_waitlist_signups_with_the_model_columns_in_order():
    table_name, *elements = _create_table_call().args
    assert table_name == "waitlist_signups"
    column_names = [
        element.name for element in elements if isinstance(element, sa.Column)
    ]
    assert column_names == ["id", "email", "sa_orm_sentinel", "created_at", "updated_at"]
    assert column_names == list(WaitlistSignup.__table__.columns.keys())


def test_upgrade_names_the_primary_key_and_unique_constraints():
    _, *elements = _create_table_call().args
    constraint_names = {
        element.name for element in elements if isinstance(element, sa.Constraint)
    }
    assert "pk_waitlist_signups" in constraint_names
    assert "uq_waitlist_signups_email" in constraint_names


def test_downgrade_drops_the_table():
    migration = _load_migration()
    with patch.object(migration.op, "drop_table") as drop_table:
        migration.downgrade()
    drop_table.assert_called_once_with("waitlist_signups")


def test_model_email_column_is_unique_and_capped_at_the_normalizer_maximum():
    email_column = WaitlistSignup.__table__.columns["email"]
    assert email_column.unique is True
    assert email_column.nullable is False
    assert email_column.type.length == MAX_WAITLIST_ADDRESS_LENGTH
