"""devices.status nullable, and cleared

Revision ID: 7c34bee528bd
Revises: d1f2a3b4c5e6
Create Date: 2026-09-26 03:00:00

Presence is derived from ``last_seen_at`` (buildapp/presence.py) and pending from
``owner_user_id``; nothing reads ``devices.status`` and the app no longer writes it.
Step one of two: the column stays so the previous image, which still selects and
inserts it, keeps working through the rollout; it goes NULL so the image that no
longer maps it can insert, and cleared so no row claims a liveness nobody keeps (a
Mac mini read "online" days after its last heartbeat). A later revision drops it.
"""

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

# revision identifiers, used by Alembic.
revision: str = "7c34bee528bd"
down_revision: Union[str, None] = "d1f2a3b4c5e6"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Batch, so the dev SQLite database can alter a column too; Postgres gets a
    # plain ALTER.
    with op.batch_alter_table("devices") as batch:
        batch.alter_column("status", existing_type=sa.String(length=20), nullable=True)
    op.execute("UPDATE devices SET status = NULL")


def downgrade() -> None:
    # Refill what the old writers would have left: "pending" until a device is
    # claimed, "offline" after (its liveness was never the column's to tell).
    op.execute(
        "UPDATE devices SET status = CASE"
        " WHEN approved = false AND owner_user_id IS NULL THEN 'pending'"
        " ELSE 'offline' END"
        " WHERE status IS NULL"
    )
    with op.batch_alter_table("devices") as batch:
        batch.alter_column("status", existing_type=sa.String(length=20), nullable=False)
