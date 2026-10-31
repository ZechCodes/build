"""Separate account-bound endpoint, challenge, family and replay authority."""

import sqlalchemy as sa
from alembic import op
from advanced_alchemy.types import GUID, DateTimeUTC

revision = "a910b2c3d4e5"
down_revision = "e2a4b6c8d0f1"
branch_labels = None
depends_on = None


def upgrade():
    with op.batch_alter_table("devices") as batch:
        batch.add_column(
            sa.Column(
                "authorization_epoch", sa.Integer(), nullable=False, server_default="0"
            )
        )
    op.create_table(
        "bridge_http_endpoints",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("device_id", GUID(length=16), nullable=False),
        sa.Column("user_id", GUID(length=16), nullable=False),
        sa.Column("origin", sa.String(2048), nullable=False),
        sa.Column("identity_fingerprint", sa.String(64), nullable=False),
        sa.Column("endpoint_epoch", sa.String(128), nullable=False),
        sa.Column("authorization_epoch", sa.Integer(), nullable=False),
        sa.Column("enabled", sa.Boolean(), nullable=False),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name="pk_bridge_http_endpoints"),
        sa.ForeignKeyConstraint(
            ["user_id"],
            ["users.id"],
            name="fk_bridge_http_endpoints_user_id_users",
            ondelete="CASCADE",
        ),
        sa.ForeignKeyConstraint(
            ["device_id"],
            ["devices.id"],
            name="fk_bridge_http_endpoints_device_id_devices",
            ondelete="CASCADE",
        ),
        sa.UniqueConstraint("device_id", name="uq_bridge_http_endpoints_device_id"),
    )
    op.create_index(
        "ix_bridge_http_endpoints_user_id",
        "bridge_http_endpoints",
        ["user_id"],
        unique=False,
    )
    op.create_table(
        "bridge_http_challenges",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("purpose", sa.String(32), nullable=False),
        sa.Column("device_id", GUID(length=16), nullable=True),
        sa.Column("user_id", GUID(length=16), nullable=True),
        sa.Column("client_binding", sa.String(128), nullable=True),
        sa.Column("origin", sa.String(2048), nullable=True),
        sa.Column("nonce", sa.String(128), nullable=False),
        sa.Column("nonce_hash", sa.String(64), nullable=False),
        sa.Column("expires_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("consumed_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name="pk_bridge_http_challenges"),
        sa.ForeignKeyConstraint(
            ["user_id"],
            ["users.id"],
            name="fk_bridge_http_challenges_user_id_users",
            ondelete="CASCADE",
        ),
        sa.ForeignKeyConstraint(
            ["device_id"],
            ["devices.id"],
            name="fk_bridge_http_challenges_device_id_devices",
            ondelete="CASCADE",
        ),
    )
    op.create_index(
        "ix_bridge_http_challenges_device_id",
        "bridge_http_challenges",
        ["device_id"],
        unique=False,
    )
    op.create_index(
        "ix_bridge_http_challenges_expires_at",
        "bridge_http_challenges",
        ["expires_at"],
        unique=False,
    )
    op.create_index(
        "ix_bridge_http_challenges_user_id",
        "bridge_http_challenges",
        ["user_id"],
        unique=False,
    )
    op.create_table(
        "bridge_http_families",
        sa.Column("issuer", sa.String(2048), nullable=False),
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("user_id", GUID(length=16), nullable=False),
        sa.Column("device_id", GUID(length=16), nullable=False),
        sa.Column("client_binding", sa.String(128), nullable=False),
        sa.Column("origin", sa.String(2048), nullable=False),
        sa.Column("endpoint_epoch", sa.String(128), nullable=False),
        sa.Column("authorization_epoch", sa.Integer(), nullable=False),
        sa.Column("scope", sa.String(128), nullable=False),
        sa.Column("parent_jti", sa.String(128), nullable=True),
        sa.Column("parent_expires_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("revoked_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name="pk_bridge_http_families"),
        sa.ForeignKeyConstraint(
            ["device_id"],
            ["devices.id"],
            name="fk_bridge_http_families_device_id_devices",
            ondelete="CASCADE",
        ),
        sa.ForeignKeyConstraint(
            ["user_id"],
            ["users.id"],
            name="fk_bridge_http_families_user_id_users",
            ondelete="CASCADE",
        ),
    )
    op.create_index(
        "ix_bridge_http_families_device_id",
        "bridge_http_families",
        ["device_id"],
        unique=False,
    )
    op.create_index(
        "ix_bridge_http_families_user_id",
        "bridge_http_families",
        ["user_id"],
        unique=False,
    )
    op.create_table(
        "bridge_http_tokens",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("family_id", GUID(length=16), nullable=False),
        sa.Column("token_hash", sa.String(64), nullable=False),
        sa.Column("expires_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name="pk_bridge_http_tokens"),
        sa.ForeignKeyConstraint(
            ["family_id"],
            ["bridge_http_families.id"],
            name="fk_bridge_http_tokens_family_id_bridge_http_families",
            ondelete="CASCADE",
        ),
    )
    op.create_index(
        "ix_bridge_http_tokens_expires_at",
        "bridge_http_tokens",
        ["expires_at"],
        unique=False,
    )
    op.create_index(
        "ix_bridge_http_tokens_family_id",
        "bridge_http_tokens",
        ["family_id"],
        unique=False,
    )
    op.create_index(
        "ix_bridge_http_tokens_token_hash",
        "bridge_http_tokens",
        ["token_hash"],
        unique=True,
    )
    op.create_table(
        "bridge_http_replays",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("device_id", GUID(length=16), nullable=False),
        sa.Column("jti", sa.String(128), nullable=False),
        sa.Column("expires_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name="pk_bridge_http_replays"),
        sa.ForeignKeyConstraint(
            ["device_id"],
            ["devices.id"],
            name="fk_bridge_http_replays_device_id_devices",
            ondelete="CASCADE",
        ),
        sa.UniqueConstraint("device_id", "jti", name="uq_bridge_http_device_jti"),
    )
    op.create_index(
        "ix_bridge_http_replays_device_id",
        "bridge_http_replays",
        ["device_id"],
        unique=False,
    )
    op.create_index(
        "ix_bridge_http_replays_expires_at",
        "bridge_http_replays",
        ["expires_at"],
        unique=False,
    )
    op.create_table(
        "bridge_http_clients",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("user_id", GUID(length=16), nullable=False),
        sa.Column("binding_hash", sa.String(64), nullable=False),
        sa.Column("revoked_at", DateTimeUTC(timezone=True), nullable=True),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name="pk_bridge_http_clients"),
        sa.ForeignKeyConstraint(
            ["user_id"],
            ["users.id"],
            name="fk_bridge_http_clients_user_id_users",
            ondelete="CASCADE",
        ),
        sa.UniqueConstraint(
            "user_id", "binding_hash", name="uq_bridge_http_client_binding"
        ),
    )
    op.create_index(
        "ix_bridge_http_clients_user_id",
        "bridge_http_clients",
        ["user_id"],
        unique=False,
    )


def downgrade():
    op.drop_table("bridge_http_clients")
    op.drop_table("bridge_http_replays")
    op.drop_table("bridge_http_tokens")
    op.drop_table("bridge_http_families")
    op.drop_table("bridge_http_challenges")
    op.drop_table("bridge_http_endpoints")
    with op.batch_alter_table("devices") as batch:
        batch.drop_column("authorization_epoch")
