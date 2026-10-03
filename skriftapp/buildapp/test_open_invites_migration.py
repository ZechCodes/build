"""Execute the additive invite-kind/consent migration against legacy SQLite rows."""
from datetime import datetime, timezone
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
        sa.Table("users", sa.MetaData(), sa.Column("id", GUID(length=16), primary_key=True)).create(conn)
        with Operations.context(MigrationContext.configure(conn)):
            load_migration(ORIGINAL).upgrade()
        conn.execute(sa.text("INSERT INTO invites (id, token_hash, email, expires_at, created_at, updated_at) VALUES (:id, 'legacy', 'legacy@example.com', :now, :now, :now)"), {"id": old_id.bytes, "now": now})
        with Operations.context(MigrationContext.configure(conn)):
            load_migration(MIGRATION).upgrade()
        rows = conn.execute(sa.text("SELECT email, kind FROM invites")).all()
        assert rows == [("legacy@example.com", "email_bound")]
        columns = {c["name"]: c for c in sa.inspect(conn).get_columns("invites")}
        assert not columns["email"]["nullable"] and not columns["kind"]["nullable"]
        assert "user_email_preferences" in sa.inspect(conn).get_table_names()
        prefs = sa.Table("user_email_preferences", sa.MetaData(), autoload_with=conn)
        conn.execute(sa.text("INSERT INTO user_email_preferences (id, user_id, created_at, updated_at) VALUES (:id, :user_id, :now, :now)"), {"id": uuid4().bytes, "user_id": uuid4().bytes, "now": now})
        preference = conn.execute(sa.select(prefs.c.product_email_opt_in, prefs.c.product_email_opted_in_at)).one()
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
        assert "kind" not in {c["name"] for c in sa.inspect(conn).get_columns("invites")}
        assert "user_email_preferences" not in sa.inspect(conn).get_table_names()
