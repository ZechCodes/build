"""Explicit invitation kinds and per-user product-email consent.

Revision ID: e2a4b6c8d0f1
Revises: 198bbccf0ca3

Existing invites remain email-bound. Unclaimed open links alone may have an empty
address. Existing users gain no implicit email consent. Downgrading preserves the
invite rows and revokes unclaimed open links before the older app loses the
kind distinction.
"""

import sqlalchemy as sa
from advanced_alchemy.types import GUID, DateTimeUTC
from alembic import op

revision = "e2a4b6c8d0f1"
down_revision = "198bbccf0ca3"
branch_labels = None
depends_on = None


def upgrade() -> None:
    with op.batch_alter_table("invites") as batch:
        batch.add_column(
            sa.Column(
                "kind", sa.String(20), nullable=False, server_default="email_bound"
            )
        )
    # Downgrade retains blank-address open links after revoking them. On a later
    # upgrade, the new column defaults every legacy row to email_bound; restore
    # the kind of only those safely revoked, unclaimed blank-address rows before
    # enforcing the address constraint. A live blank-address row remains invalid.
    op.execute(
        sa.text("""UPDATE invites SET kind = 'open_link'
        WHERE email = '' AND revoked_at IS NOT NULL
          AND redeemed_at IS NULL AND redeemed_by IS NULL""")
    )
    with op.batch_alter_table("invites") as batch:
        batch.create_check_constraint(
            "ck_invites_invite_kind", "kind IN ('email_bound', 'open_link')"
        )
        batch.create_check_constraint(
            "ck_invites_invite_address",
            "email <> '' OR (kind = 'open_link' AND redeemed_at IS NULL AND redeemed_by IS NULL)",
        )
    op.create_table(
        "user_email_preferences",
        sa.Column("id", GUID(length=16), nullable=False),
        sa.Column("user_id", GUID(length=16), nullable=False),
        sa.Column(
            "product_email_opt_in",
            sa.Boolean(),
            nullable=False,
            server_default=sa.false(),
        ),
        sa.Column(
            "product_email_opted_in_at", DateTimeUTC(timezone=True), nullable=True
        ),
        sa.Column("sa_orm_sentinel", sa.Integer(), nullable=True),
        sa.Column("created_at", DateTimeUTC(timezone=True), nullable=False),
        sa.Column("updated_at", DateTimeUTC(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id", name="pk_user_email_preferences"),
        sa.UniqueConstraint("user_id", name="uq_user_email_preferences_user_id"),
        sa.ForeignKeyConstraint(
            ["user_id"],
            ["users.id"],
            name="fk_user_email_preferences_user_id_users",
            ondelete="CASCADE",
        ),
    )


def downgrade() -> None:
    # The previous app matches invite.email directly. Revoke only unclaimed
    # open links before dropping their kind; leave redeemed and previously
    # revoked rows untouched.
    op.execute(
        sa.text("""UPDATE invites
        SET revoked_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE kind = 'open_link'
          AND redeemed_at IS NULL AND redeemed_by IS NULL AND revoked_at IS NULL""")
    )
    op.drop_table("user_email_preferences")
    with op.batch_alter_table("invites") as batch:
        batch.drop_constraint("ck_invites_invite_address", type_="check")
        batch.drop_constraint("ck_invites_invite_kind", type_="check")
        batch.drop_column("kind")
