"""Add dormant Build account policy and the pairing's first-ready latch."""

import sqlalchemy as sa
from advanced_alchemy.types import DateTimeUTC, GUID
from alembic import op

revision = "b2c3d4e5f6a7"
down_revision = "a910b2c3d4e5"
branch_labels = None
depends_on = None


def audit_columns():
    return (
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
    )


def upgrade():
    op.add_column("devices", sa.Column(
        "hosted_ready_once", sa.Boolean(), nullable=False, server_default=sa.false(),
    ))
    op.create_table(
        "account_policies",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("account_type", sa.String(5), nullable=False, server_default="free"),
        sa.Column(
            "tunnel_access",
            sa.Boolean(
                create_constraint=True,
                name=op.f("ck_account_policies_account_policy_tunnel_access"),
            ),
            nullable=False, server_default=sa.false(),
        ),
        sa.Column("promotion_revision", sa.Integer(), nullable=False, server_default="0"),
        *audit_columns(),
        sa.CheckConstraint(
            "account_type IN ('free', 'admin')",
            name=op.f("ck_account_policies_account_policy_type"),
        ),
        sa.CheckConstraint(
            "promotion_revision >= 0",
            name=op.f("ck_account_policies_account_policy_revision"),
        ),
        sa.ForeignKeyConstraint(
            ["id"], ["users.id"], name="fk_account_policies_id_users", ondelete="CASCADE",
        ),
        sa.PrimaryKeyConstraint("id", name="pk_account_policies"),
    )
    op.create_table(
        "account_policy_audit",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("target_user_id", GUID(length=16), nullable=False),
        sa.Column("actor", sa.String(512), nullable=False),
        sa.Column("old_policy", sa.JSON(), nullable=False),
        sa.Column("new_policy", sa.JSON(), nullable=False),
        *audit_columns(),
        sa.PrimaryKeyConstraint("id", name="pk_account_policy_audit"),
    )
    op.create_index("ix_account_policy_audit_target_user_id", "account_policy_audit", ["target_user_id"])


def downgrade():
    op.drop_table("account_policy_audit")
    op.drop_table("account_policies")
    op.drop_column("devices", "hosted_ready_once")
