#!/bin/sh
# Dev-stack entrypoint for the app container (mounted by deploy/compose.real.yml).
# Same migrate-then-serve shape as the production entrypoint, plus a one-time,
# idempotent seed of Skrift's first-run setup so dummy login works immediately,
# and of one open invite so the QA user can get past the alpha gate.
set -eu

skrift db upgrade heads

BUILD_DEV_INVITE_TOKEN="${BUILD_DEV_INVITE_TOKEN:-COMPOSE-INVITE}"
BUILD_DEV_INVITE_EMAIL="${BUILD_DEV_INVITE_EMAIL:-qa@localhost}"
export BUILD_DEV_INVITE_TOKEN BUILD_DEV_INVITE_EMAIL

python - <<'PY'
import datetime
import hashlib
import os
import sqlite3
import uuid

db = sqlite3.connect("app.db")
now = datetime.datetime.now(datetime.UTC)
stamp = now.isoformat()

already_seeded = db.execute(
    "SELECT 1 FROM settings WHERE key = 'setup_completed_at'"
).fetchone()
if not already_seeded:
    db.execute(
        "INSERT INTO settings (id, key, value, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        (uuid.uuid4().bytes, "setup_completed_at", "done", stamp, stamp),
    )
    print("seeded settings.setup_completed_at (first-run setup skipped)")

# Build is invite-only; the dev stack seeds one open invite so `podman compose
# run qa` (and a human poking at http://localhost:8090/app/) can redeem it.
# Same rule the app uses: the row keeps the SHA-256 hex of the raw token.
invite_token = os.environ["BUILD_DEV_INVITE_TOKEN"]
invite_email = os.environ["BUILD_DEV_INVITE_EMAIL"]
token_hash = hashlib.sha256(invite_token.encode("utf-8")).hexdigest()
already_invited = db.execute(
    "SELECT 1 FROM invites WHERE token_hash = ?", (token_hash,)
).fetchone()
if not already_invited:
    db.execute(
        "INSERT INTO invites (id, token_hash, email, invited_by, expires_at,"
        " redeemed_by, redeemed_at, revoked_at, created_at, updated_at)"
        " VALUES (?, ?, ?, NULL, ?, NULL, NULL, NULL, ?, ?)",
        (
            uuid.uuid4().bytes,
            token_hash,
            invite_email,
            (now + datetime.timedelta(days=3650)).isoformat(),
            stamp,
            stamp,
        ),
    )
    print(f"seeded an open invite for {invite_email} at /invite/{invite_token}")

db.commit()
db.close()
PY

exec skrift serve --host "${HOST:-0.0.0.0}" --port "${PORT:-8080}"
