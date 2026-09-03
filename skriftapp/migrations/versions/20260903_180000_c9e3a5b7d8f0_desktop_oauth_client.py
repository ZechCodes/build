"""desktop_oauth_client

Revision ID: c9e3a5b7d8f0
Revises: b8d2f4a6c7e9
Create Date: 2026-09-03 18:00:00
"""

from collections.abc import Sequence
from uuid import UUID

import sqlalchemy as sa
from alembic import op

revision: str = "c9e3a5b7d8f0"
down_revision: str | None = "b8d2f4a6c7e9"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None

DESKTOP_CLIENT_DATABASE_ID = UUID("ac297ec5-3881-4df7-a9b8-35b20be6b718")


def upgrade() -> None:
    clients = sa.table(
        "oauth2_clients",
        sa.column("id", sa.Uuid()),
        sa.column("client_id", sa.String()),
        sa.column("client_secret", sa.String()),
        sa.column("display_name", sa.String()),
        sa.column("redirect_uris", sa.Text()),
        sa.column("allowed_scopes", sa.Text()),
        sa.column("is_active", sa.Boolean()),
    )
    op.bulk_insert(
        clients,
        [
            {
                "id": DESKTOP_CLIENT_DATABASE_ID,
                "client_id": "build-desktop",
                "client_secret": "",
                "display_name": "Build Desktop",
                "redirect_uris": "getbuilding://oauth/callback",
                "allowed_scopes": "openid\nprofile\nemail",
                "is_active": True,
            }
        ],
    )


def downgrade() -> None:
    op.execute(
        sa.text("DELETE FROM oauth2_clients WHERE client_id = 'build-desktop'")
    )
