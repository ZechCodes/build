"""waitlist_signups

Revision ID: b8d2f4a6c7e9
Revises: a7c1e2f3b4d5
Create Date: 2026-09-01 12:00:00

Additive-only: one table holding one row per email address the prelaunch landing
page collected for the private beta.
"""

from collections.abc import Sequence

import sqlalchemy as sa
from advanced_alchemy.types import GUID, DateTimeUTC
from alembic import op

revision: str = "b8d2f4a6c7e9"
down_revision: str | None = "a7c1e2f3b4d5"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.create_table(
        "waitlist_signups",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("email", sa.String(length=254), nullable=False),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_waitlist_signups")),
        sa.UniqueConstraint("email", name=op.f("uq_waitlist_signups_email")),
    )


def downgrade() -> None:
    op.drop_table("waitlist_signups")
