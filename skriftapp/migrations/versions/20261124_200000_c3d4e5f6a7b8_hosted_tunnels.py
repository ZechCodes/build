"""Hosted tunnel assignments and provisioner wakeups."""

import sqlalchemy as sa
from advanced_alchemy.types import DateTimeUTC, GUID
from alembic import op

revision = "c3d4e5f6a7b8"
down_revision = "b2c3d4e5f6a7"
branch_labels = None
depends_on = None

SOURCES = {
    "devices": "approved, owner_user_id, authorization_epoch",
    "account_policies": "tunnel_access",
    "invites": "redeemed_by, revoked_at",
    "users": "is_active",
    "user_roles": "user_id, role_id",
    "role_permissions": "role_id, permission",
}


def upgrade():
    op.create_table(
        "hosted_tunnel_assignments",
        sa.Column("device_id", GUID(length=16), nullable=False),
        sa.Column("owner_user_id", GUID(length=16), nullable=False),
        sa.Column("pairing_created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("authorization_epoch", sa.Integer(), nullable=False),
        sa.Column("generation", sa.BigInteger(), nullable=False),
        sa.Column("label", sa.String(32), nullable=False),
        sa.Column("hostname", sa.String(253), nullable=False),
        sa.Column("tunnel_id", GUID(length=16), nullable=True),
        sa.Column("dns_record_id", sa.String(64), nullable=True),
        sa.Column("tunnel_create_started", sa.Boolean(), nullable=False),
        sa.Column("state", sa.String(8), nullable=False),
        sa.Column("setup_reason", sa.String(32), nullable=True),
        sa.Column("encrypted_secret", sa.LargeBinary(), nullable=False),
        sa.PrimaryKeyConstraint("device_id", name="pk_hosted_tunnel_assignments"),
        sa.UniqueConstraint("label", name="uq_hosted_tunnel_assignments_label"),
        sa.UniqueConstraint("hostname", name="uq_hosted_tunnel_assignments_hostname"),
        sa.UniqueConstraint("tunnel_id", name="uq_hosted_tunnel_assignments_tunnel_id"),
        sa.UniqueConstraint("dns_record_id", name="uq_hosted_tunnel_assignments_dns_record_id"),
        sa.CheckConstraint("generation > 0", name=op.f("ck_hosted_tunnel_assignments_positive_generation")),
        sa.CheckConstraint(
            "state IN ('pending', 'ready', 'removing')",
            name=op.f("ck_hosted_tunnel_assignments_known_state"),
        ),
        sa.CheckConstraint(
            "state != 'ready' OR (tunnel_id IS NOT NULL AND dns_record_id IS NOT NULL)",
            name=op.f("ck_hosted_tunnel_assignments_ready_has_provider_ids"),
        ),
    )
    if op.get_bind().dialect.name == "postgresql":
        op.execute("""
            CREATE FUNCTION build_wake_hosted_tunnels() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN
                PERFORM pg_notify('build_hosted_tunnels_changed', '');
                RETURN NULL;
            END;
            $$
        """)
        for table, columns in SOURCES.items():
            op.execute(f"""
                CREATE TRIGGER wake_hosted_tunnels
                AFTER INSERT OR DELETE OR UPDATE OF {columns} ON {table}
                FOR EACH STATEMENT EXECUTE FUNCTION build_wake_hosted_tunnels()
            """)


def downgrade():
    if op.get_bind().dialect.name == "postgresql":
        for table in SOURCES:
            op.execute(f"DROP TRIGGER wake_hosted_tunnels ON {table}")
        op.execute("DROP FUNCTION build_wake_hosted_tunnels()")
    op.drop_table("hosted_tunnel_assignments")
