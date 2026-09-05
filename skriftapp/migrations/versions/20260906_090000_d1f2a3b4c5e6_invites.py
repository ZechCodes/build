"""invites

Revision ID: d1f2a3b4c5e6
Revises: 20bec806d0e5
Create Date: 2026-09-06 09:00:00

Additive-only: one row per alpha invitation. The redeemed, unrevoked row is the
alpha-membership marker (buildapp/alpha_membership.py) — no separate members table.
"""

from typing import Sequence, Union

import sqlalchemy as sa
from advanced_alchemy.types import GUID, DateTimeUTC
from alembic import op

# revision identifiers, used by Alembic.
revision: str = "d1f2a3b4c5e6"
down_revision: Union[str, None] = "20bec806d0e5"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "invites",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("token_hash", sa.String(length=64), nullable=False),
        # 254, the width waitlist_address.MAX_WAITLIST_ADDRESS_LENGTH had when this
        # revision was written. A migration is a snapshot: it never reads live code.
        sa.Column("email", sa.String(length=254), nullable=False),
        sa.Column("invited_by", GUID(length=16), nullable=True),
        sa.Column("expires_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("redeemed_by", GUID(length=16), nullable=True),
        sa.Column("redeemed_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("revoked_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_invites")),
        sa.UniqueConstraint("token_hash", name="uq_invites_token_hash"),
        sa.ForeignKeyConstraint(
            ["invited_by"],
            ["users.id"],
            name="fk_invites_invited_by_users",
            ondelete="SET NULL",
        ),
        sa.ForeignKeyConstraint(
            ["redeemed_by"],
            ["users.id"],
            name="fk_invites_redeemed_by_users",
            ondelete="CASCADE",
        ),
    )
    op.create_index(op.f("ix_invites_token_hash"), "invites", ["token_hash"])
    op.create_index(op.f("ix_invites_email"), "invites", ["email"])
    op.create_index(op.f("ix_invites_invited_by"), "invites", ["invited_by"])
    op.create_index(op.f("ix_invites_redeemed_by"), "invites", ["redeemed_by"])


def downgrade() -> None:
    op.drop_index(op.f("ix_invites_redeemed_by"), table_name="invites")
    op.drop_index(op.f("ix_invites_invited_by"), table_name="invites")
    op.drop_index(op.f("ix_invites_email"), table_name="invites")
    op.drop_index(op.f("ix_invites_token_hash"), table_name="invites")
    op.drop_table("invites")
