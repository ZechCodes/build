#!/bin/sh
# Dev-stack entrypoint for the app container (mounted by deploy/compose.real.yml).
# Same migrate-then-serve shape as the production entrypoint, plus a one-time,
# idempotent seed of Skrift's first-run setup so dummy login works immediately.
set -eu

skrift db upgrade heads

python - <<'PY'
import datetime
import sqlite3
import uuid

db = sqlite3.connect("app.db")
already_seeded = db.execute(
    "SELECT 1 FROM settings WHERE key = 'setup_completed_at'"
).fetchone()
if not already_seeded:
    now = datetime.datetime.now(datetime.UTC).isoformat()
    db.execute(
        "INSERT INTO settings (id, key, value, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        (uuid.uuid4().bytes, "setup_completed_at", "done", now, now),
    )
    db.commit()
    print("seeded settings.setup_completed_at (first-run setup skipped)")
db.close()
PY

exec skrift serve --host "${HOST:-0.0.0.0}" --port "${PORT:-8080}"
