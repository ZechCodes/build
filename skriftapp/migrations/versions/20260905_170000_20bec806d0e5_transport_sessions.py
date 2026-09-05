"""transport_sessions

Revision ID: 20bec806d0e5
Revises: c9e3a5b7d8f0
Create Date: 2026-09-05 17:00:00

Additive-only: one row per client session's transport life, as the bridge
reports it (planning/v2/Transport Telemetry Spec.md). Content-free — ids,
timestamps, and a word for the path — read by the admin transport page.
"""

from typing import Sequence, Union

import sqlalchemy as sa
from advanced_alchemy.types import GUID, DateTimeUTC
from alembic import op

# revision identifiers, used by Alembic.
revision: str = "20bec806d0e5"
down_revision: Union[str, None] = "c9e3a5b7d8f0"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "transport_sessions",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("session_id", sa.String(length=64), nullable=False),
        sa.Column("device_id", GUID(length=16), nullable=False),
        sa.Column("owner_user_id", GUID(length=16), nullable=True),
        sa.Column("minted_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("first_carrying_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("first_path", sa.String(length=16), nullable=True),
        sa.Column("current_path", sa.String(length=16), nullable=False),
        sa.Column("carrying_count", sa.Integer(), nullable=False),
        sa.Column("turn_count", sa.Integer(), nullable=False),
        sa.Column("fell_back_count", sa.Integer(), nullable=False),
        sa.Column("ended_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_transport_sessions")),
        sa.UniqueConstraint(
            "device_id", "session_id", name="uq_transport_sessions_device_session"
        ),
    )
    op.create_index(
        op.f("ix_transport_sessions_session_id"), "transport_sessions", ["session_id"]
    )
    op.create_index(
        op.f("ix_transport_sessions_device_id"), "transport_sessions", ["device_id"]
    )
    op.create_index(
        op.f("ix_transport_sessions_owner_user_id"),
        "transport_sessions",
        ["owner_user_id"],
    )
    op.create_index(
        op.f("ix_transport_sessions_minted_at"), "transport_sessions", ["minted_at"]
    )


def downgrade() -> None:
    op.drop_index(op.f("ix_transport_sessions_minted_at"), table_name="transport_sessions")
    op.drop_index(op.f("ix_transport_sessions_owner_user_id"), table_name="transport_sessions")
    op.drop_index(op.f("ix_transport_sessions_device_id"), table_name="transport_sessions")
    op.drop_index(op.f("ix_transport_sessions_session_id"), table_name="transport_sessions")
    op.drop_table("transport_sessions")
