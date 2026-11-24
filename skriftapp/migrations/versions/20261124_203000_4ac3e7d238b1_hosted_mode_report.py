"""Persist signed hosted mode and record the generation of hosted endpoints."""

import sqlalchemy as sa
from advanced_alchemy.types import DateTimeUTC, GUID
from alembic import op

revision = "4ac3e7d238b1"
down_revision = "c3d4e5f6a7b8"
branch_labels = None
depends_on = None


def upgrade():
    op.add_column(
        "bridge_http_endpoints", sa.Column("assignment_generation", sa.BigInteger(), nullable=True),
    )
    op.create_table(
        "hosted_mode_reports",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("device_id", GUID(length=16), nullable=False),
        sa.Column("owner_user_id", GUID(length=16), nullable=False),
        sa.Column("pairing_created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("authorization_epoch", sa.Integer(), nullable=False),
        sa.Column("mode", sa.String(8), nullable=False),
        sa.Column("hosted_capable", sa.Boolean(), nullable=False),
        sa.Column("revision", GUID(length=16), nullable=False),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(
            ["device_id"], ["devices.id"], name="fk_hosted_mode_reports_device_id_devices",
            ondelete="CASCADE",
        ),
        sa.ForeignKeyConstraint(
            ["owner_user_id"], ["users.id"], name="fk_hosted_mode_reports_owner_user_id_users",
            ondelete="CASCADE",
        ),
        sa.PrimaryKeyConstraint("id", name="pk_hosted_mode_reports"),
        sa.UniqueConstraint("device_id", name="uq_hosted_mode_reports_device_id"),
    )


def downgrade():
    endpoint = sa.table(
        "bridge_http_endpoints", sa.column("device_id"), sa.column("origin"),
        sa.column("enabled", sa.Boolean()), sa.column("assignment_generation"),
    )
    family = sa.table(
        "bridge_http_families", sa.column("device_id"), sa.column("origin"), sa.column("revoked_at"),
    )
    hosted = endpoint.c.assignment_generation.is_not(None)
    # The previous revision cannot distinguish a hosted endpoint from a
    # personal address. Withdraw it and its families before dropping the marker.
    matching_origin = sa.select(1).select_from(endpoint).where(
        hosted, endpoint.c.device_id == family.c.device_id, endpoint.c.origin == family.c.origin,
    ).exists()
    op.execute(
        family.update().where(family.c.revoked_at.is_(None), matching_origin)
        .values(revoked_at=sa.func.now())
    )
    op.execute(endpoint.update().where(hosted).values(enabled=False))
    op.drop_table("hosted_mode_reports")
    op.drop_column("bridge_http_endpoints", "assignment_generation")
