"""Tests pinning the invites migration to the model it materialises: the revision
chain, the column order, the named constraints, and the indexes every invite lookup
(by token, by address, by inviter, by redeemer) reads."""

from __future__ import annotations

import sqlalchemy as sa

from buildapp.migration_test_support import (
    downgrade_calls,
    load_migration,
    migration_path,
    table_elements,
    upgrade_calls,
)
from buildapp.models import Invite
from buildapp.waitlist_address import MAX_WAITLIST_ADDRESS_LENGTH

_MIGRATION_PATH = migration_path("20260906_090000_d1f2a3b4c5e6_invites.py")


def test_revision_chains_from_the_transport_sessions_migration():
    migration = load_migration(_MIGRATION_PATH)
    assert migration.revision == "d1f2a3b4c5e6"
    assert migration.down_revision == "20bec806d0e5"


def test_upgrade_creates_the_original_invite_columns_in_order():
    table_name, elements = table_elements(_MIGRATION_PATH)
    assert table_name == "invites"
    column_names = [e.name for e in elements if isinstance(e, sa.Column)]
    # The later open-links revision appends kind; this revision remains frozen.
    assert column_names == [name for name in Invite.__table__.columns.keys() if name != "kind"]


def test_upgrade_names_the_primary_key_the_token_uniqueness_and_both_user_keys():
    _, elements = table_elements(_MIGRATION_PATH)
    constraint_names = {e.name for e in elements if isinstance(e, sa.Constraint)}
    assert "pk_invites" in constraint_names
    assert "uq_invites_token_hash" in constraint_names
    assert "fk_invites_invited_by_users" in constraint_names
    assert "fk_invites_redeemed_by_users" in constraint_names


def test_upgrade_indexes_every_column_an_invite_is_looked_up_by():
    _, index_calls = upgrade_calls(_MIGRATION_PATH)
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
    drop_table, drop_index = downgrade_calls(_MIGRATION_PATH)
    drop_table.assert_called_once_with("invites")
    assert drop_index.call_count == 4


def test_the_email_column_is_as_wide_as_the_waitlist_accepts():
    _, elements = table_elements(_MIGRATION_PATH)
    email = next(e for e in elements if isinstance(e, sa.Column) and e.name == "email")
    assert email.type.length == MAX_WAITLIST_ADDRESS_LENGTH


def test_the_revision_reads_nothing_from_the_running_application():
    """A revision is a frozen snapshot. The test may read the width from the app; the
    migration may not, or a later change to the constant silently rewrites what this
    revision creates on a fresh database while every existing one stays as it was."""
    imports = [
        line
        for line in _MIGRATION_PATH.read_text().splitlines()
        if line.startswith(("import ", "from ")) and "buildapp" in line
    ]
    assert imports == []
