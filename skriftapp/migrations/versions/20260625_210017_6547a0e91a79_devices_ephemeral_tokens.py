"""devices + ephemeral_tokens

Revision ID: 6547a0e91a79
Revises: f2a3b4c5d6e7
Create Date: 2026-06-25 21:00:17

Build-owned, additive-only migration: creates the `devices` and `ephemeral_tokens`
tables for device pairing. Autogenerate also emitted spurious NUMERIC/CHAR→GUID
alter_columns and constraint drops across framework tables (a SQLite GUID-reflection
false positive that would have dropped real unique constraints); those were removed —
this migration only adds the two new tables.
"""

from typing import Sequence, Union

import sqlalchemy as sa
from advanced_alchemy.types import GUID, DateTimeUTC
from alembic import op

# revision identifiers, used by Alembic.
revision: str = "6547a0e91a79"
down_revision: Union[str, None] = "f2a3b4c5d6e7"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "devices",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("name", sa.String(length=255), nullable=False),
        sa.Column("owner_user_id", GUID(length=16), nullable=True),
        sa.Column("identity_public_key_b64", sa.String(length=128), nullable=False),
        sa.Column("transport_public_key_b64", sa.String(length=128), nullable=False),
        sa.Column("pairing_code_hash", sa.String(length=64), nullable=True),
        sa.Column("approved", sa.Boolean(), nullable=False),
        sa.Column("status", sa.String(length=20), nullable=False),
        sa.Column("last_seen_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(
            ["owner_user_id"],
            ["users.id"],
            name=op.f("fk_devices_owner_user_id_users"),
            ondelete="CASCADE",
        ),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_devices")),
    )
    op.create_index(
        op.f("ix_devices_owner_user_id"), "devices", ["owner_user_id"], unique=False
    )
    op.create_index(
        op.f("ix_devices_pairing_code_hash"), "devices", ["pairing_code_hash"], unique=False
    )

    op.create_table(
        "ephemeral_tokens",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("token_hash", sa.String(length=64), nullable=False),
        sa.Column("purpose", sa.String(length=50), nullable=False),
        sa.Column("user_id", GUID(length=16), nullable=False),
        sa.Column("expires_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(
            ["user_id"],
            ["users.id"],
            name=op.f("fk_ephemeral_tokens_user_id_users"),
            ondelete="CASCADE",
        ),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_ephemeral_tokens")),
    )
    op.create_index(
        op.f("ix_ephemeral_tokens_expires_at"), "ephemeral_tokens", ["expires_at"], unique=False
    )
    op.create_index(
        op.f("ix_ephemeral_tokens_token_hash"), "ephemeral_tokens", ["token_hash"], unique=True
    )
    op.create_index(
        op.f("ix_ephemeral_tokens_user_id"), "ephemeral_tokens", ["user_id"], unique=False
    )


def downgrade() -> None:
    op.drop_index(op.f("ix_ephemeral_tokens_user_id"), table_name="ephemeral_tokens")
    op.drop_index(op.f("ix_ephemeral_tokens_token_hash"), table_name="ephemeral_tokens")
    op.drop_index(op.f("ix_ephemeral_tokens_expires_at"), table_name="ephemeral_tokens")
    op.drop_table("ephemeral_tokens")
    op.drop_index(op.f("ix_devices_pairing_code_hash"), table_name="devices")
    op.drop_index(op.f("ix_devices_owner_user_id"), table_name="devices")
    op.drop_table("devices")
