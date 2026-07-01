#!/bin/sh
# Apply pending migrations, then serve. `skrift db upgrade head` runs Skrift's
# framework migrations plus this app's migrations/versions (devices,
# ephemeral_tokens) against $DATABASE_URL from app.yaml.
set -eu

uv run --no-sync skrift db upgrade head
exec uv run --no-sync skrift serve --host "${HOST:-0.0.0.0}" --port "${PORT:-8080}"
