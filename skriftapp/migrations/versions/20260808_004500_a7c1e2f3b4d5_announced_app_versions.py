"""announced_app_versions

Revision ID: a7c1e2f3b4d5
Revises: 6547a0e91a79
Create Date: 2026-08-08 00:45:00

Additive-only: one table recording which frontend versions the startup
app-update push has already announced, so restarts of the same build never
re-notify subscribers.
"""

from typing import Sequence, Union

import sqlalchemy as sa
from advanced_alchemy.types import GUID, DateTimeUTC
from alembic import op

# revision identifiers, used by Alembic.
revision: str = "a7c1e2f3b4d5"
down_revision: Union[str, None] = "6547a0e91a79"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "announced_app_versions",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("version", sa.String(length=64), nullable=False),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_announced_app_versions")),
        sa.UniqueConstraint("version", name=op.f("uq_announced_app_versions_version")),
    )


def downgrade() -> None:
    op.drop_table("announced_app_versions")
