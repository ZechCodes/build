"""drop devices.status

Revision ID: 198bbccf0ca3
Revises: 7c34bee528bd
Create Date: 2026-09-26 03:10:00

Step two of two. Roll only after 7c34bee528bd and the image that stopped mapping
the column are live: the previous image must not select or insert it, or it fails
through the rollout (deploy/OPS.md). Presence is ``last_seen_at``, pending is
``owner_user_id``; the column carried nothing.
"""

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

# revision identifiers, used by Alembic.
revision: str = "198bbccf0ca3"
down_revision: Union[str, None] = "7c34bee528bd"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Batch, so the dev SQLite database can drop a column too.
    with op.batch_alter_table("devices") as batch:
        batch.drop_column("status")


def downgrade() -> None:
    # Back as 7c34bee528bd left it: nullable and empty.
    with op.batch_alter_table("devices") as batch:
        batch.add_column(sa.Column("status", sa.String(length=20), nullable=True))
