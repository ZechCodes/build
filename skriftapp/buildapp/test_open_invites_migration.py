"""Execute the additive invite-kind/consent migration against legacy SQLite rows."""

from datetime import datetime, timedelta, timezone
from uuid import uuid4

import pytest
import sqlalchemy as sa
from alembic.migration import MigrationContext
from alembic.operations import Operations
from advanced_alchemy.types import GUID

from buildapp.migration_test_support import load_migration, migration_path

MIGRATION = migration_path("20261003_010000_e2a4b6c8d0f1_open_invites.py")
ORIGINAL = migration_path("20260906_090000_d1f2a3b4c5e6_invites.py")


def test_revision_follows_current_app_head_and_imports_no_live_models():
    migration = load_migration(MIGRATION)
    assert migration.revision == "e2a4b6c8d0f1"
    assert migration.down_revision == "198bbccf0ca3"
    assert "buildapp" not in MIGRATION.read_text()


def test_upgrade_preserves_bound_rows_and_adds_constrained_open_links_and_consent():
    engine = sa.create_engine("sqlite://")
    now = datetime.now(timezone.utc)
    old_id = uuid4()
    with engine.begin() as conn:
        sa.Table(
            "users", sa.MetaData(), sa.Column("id", GUID(length=16), primary_key=True)
        ).create(conn)
        with Operations.context(MigrationContext.configure(conn)):
            load_migration(ORIGINAL).upgrade()
        conn.execute(
            sa.text(
                "INSERT INTO invites (id, token_hash, email, expires_at, created_at, updated_at) VALUES (:id, 'legacy', 'legacy@example.com', :now, :now, :now)"
            ),
            {"id": old_id.bytes, "now": now},
        )
        with Operations.context(MigrationContext.configure(conn)):
            load_migration(MIGRATION).upgrade()
        rows = conn.execute(sa.text("SELECT email, kind FROM invites")).all()
        assert rows == [("legacy@example.com", "email_bound")]
        columns = {c["name"]: c for c in sa.inspect(conn).get_columns("invites")}
        assert not columns["email"]["nullable"] and not columns["kind"]["nullable"]
        assert "user_email_preferences" in sa.inspect(conn).get_table_names()
        prefs = sa.Table("user_email_preferences", sa.MetaData(), autoload_with=conn)
        conn.execute(
            sa.text(
                "INSERT INTO user_email_preferences (id, user_id, created_at, updated_at) VALUES (:id, :user_id, :now, :now)"
            ),
            {"id": uuid4().bytes, "user_id": uuid4().bytes, "now": now},
        )
        preference = conn.execute(
            sa.select(prefs.c.product_email_opt_in, prefs.c.product_email_opted_in_at)
        ).one()
        assert preference.product_email_opt_in is False
        assert preference.product_email_opted_in_at is None
        with pytest.raises(sa.exc.IntegrityError):
            conn.execute(sa.text("UPDATE invites SET email = ''"))
        with pytest.raises(sa.exc.IntegrityError):
            conn.execute(sa.text("UPDATE invites SET kind = 'unknown'"))
        conn.execute(sa.text("UPDATE invites SET kind = 'open_link', email = ''"))
        with pytest.raises(sa.exc.IntegrityError):
            conn.execute(sa.text("UPDATE invites SET redeemed_at = CURRENT_TIMESTAMP"))
        with Operations.context(MigrationContext.configure(conn)):
            load_migration(MIGRATION).downgrade()
        assert "kind" not in {
            c["name"] for c in sa.inspect(conn).get_columns("invites")
        }
        assert "user_email_preferences" not in sa.inspect(conn).get_table_names()


def test_downgrade_revokes_only_unclaimed_open_links_before_dropping_kind():
    engine = sa.create_engine("sqlite://")
    now = datetime(2026, 10, 3, 1, 0, tzinfo=timezone.utc)
    old_revocation = datetime(2026, 9, 30, 12, 34, tzinfo=timezone.utc)
    ids = {name: uuid4() for name in ("unclaimed", "revoked", "bound", "redeemed")}
    redeemer_id = uuid4()
    with engine.begin() as conn:
        sa.Table(
            "users", sa.MetaData(), sa.Column("id", GUID(length=16), primary_key=True)
        ).create(conn)
        with Operations.context(MigrationContext.configure(conn)):
            load_migration(ORIGINAL).upgrade()
            load_migration(MIGRATION).upgrade()
        conn.execute(
            sa.text("""INSERT INTO invites
                (id, token_hash, kind, email, expires_at, redeemed_by, redeemed_at,
                 revoked_at, created_at, updated_at)
                VALUES (:id, :token_hash, :kind, :email, :expires_at, :redeemed_by,
                        :redeemed_at, :revoked_at, :created_at, :updated_at)"""),
            [
                dict(
                    id=ids["unclaimed"].bytes,
                    token_hash="unclaimed",
                    kind="open_link",
                    email="",
                    expires_at=now,
                    redeemed_by=None,
                    redeemed_at=None,
                    revoked_at=None,
                    created_at=now,
                    updated_at=now,
                ),
                dict(
                    id=ids["revoked"].bytes,
                    token_hash="revoked",
                    kind="open_link",
                    email="",
                    expires_at=now,
                    redeemed_by=None,
                    redeemed_at=None,
                    revoked_at=old_revocation,
                    created_at=now,
                    updated_at=now,
                ),
                dict(
                    id=ids["bound"].bytes,
                    token_hash="bound",
                    kind="email_bound",
                    email="bound@example.com",
                    expires_at=now,
                    redeemed_by=None,
                    redeemed_at=None,
                    revoked_at=None,
                    created_at=now,
                    updated_at=now,
                ),
                dict(
                    id=ids["redeemed"].bytes,
                    token_hash="redeemed",
                    kind="open_link",
                    email="redeemed@example.com",
                    expires_at=now,
                    redeemed_by=redeemer_id.bytes,
                    redeemed_at=now,
                    revoked_at=None,
                    created_at=now,
                    updated_at=now,
                ),
            ],
        )
        previous_revocation = conn.execute(
            sa.text("SELECT revoked_at FROM invites WHERE token_hash = 'revoked'")
        ).scalar_one()
        with Operations.context(MigrationContext.configure(conn)):
            load_migration(MIGRATION).downgrade()
        rows = {
            row.token_hash: row
            for row in conn.execute(
                sa.text(
                    "SELECT token_hash, email, redeemed_at, revoked_at FROM invites"
                )
            )
        }
        assert len(rows) == 4
        assert rows["unclaimed"].email == ""
        assert rows["unclaimed"].revoked_at is not None
        assert rows["revoked"].revoked_at == previous_revocation
        assert rows["bound"].email == "bound@example.com"
        assert rows["bound"].revoked_at is None
        assert rows["redeemed"].email == "redeemed@example.com"
        assert rows["redeemed"].redeemed_at is not None
        assert rows["redeemed"].revoked_at is None
        assert "kind" not in {
            column["name"] for column in sa.inspect(conn).get_columns("invites")
        }
        with Operations.context(MigrationContext.configure(conn)):
            load_migration(MIGRATION).upgrade()
        kinds = dict(
            conn.execute(sa.text("SELECT token_hash, kind FROM invites")).all()
        )
        assert kinds == {
            "unclaimed": "open_link",
            "revoked": "open_link",
            "bound": "email_bound",
            "redeemed": "email_bound",
        }
        round_trip_revocations = dict(
            conn.execute(sa.text("SELECT token_hash, revoked_at FROM invites")).all()
        )
        assert round_trip_revocations["unclaimed"] == rows["unclaimed"].revoked_at
        assert round_trip_revocations["revoked"] == previous_revocation
        revoked_at = datetime.fromisoformat(round_trip_revocations["unclaimed"])
        assert revoked_at.tzinfo is not None
        assert revoked_at.utcoffset() == timedelta(0)


def test_upgrade_rejects_a_live_legacy_blank_address_instead_of_classifying_it():
    engine = sa.create_engine("sqlite://")
    now = datetime(2026, 10, 3, 1, 0, tzinfo=timezone.utc)
    with engine.begin() as conn:
        sa.Table(
            "users", sa.MetaData(), sa.Column("id", GUID(length=16), primary_key=True)
        ).create(conn)
        with Operations.context(MigrationContext.configure(conn)):
            load_migration(ORIGINAL).upgrade()
        conn.execute(
            sa.text("""INSERT INTO invites
                (id, token_hash, email, expires_at, created_at, updated_at)
                VALUES (:id, 'unsafe-blank', '', :now, :now, :now)"""),
            {"id": uuid4().bytes, "now": now},
        )
        with pytest.raises(sa.exc.IntegrityError):
            with Operations.context(MigrationContext.configure(conn)):
                load_migration(MIGRATION).upgrade()
