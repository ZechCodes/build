#!/bin/sh
# Container entrypoint: apply migrations (Skrift core + buildapp), then serve.
set -eu

skrift db upgrade heads
# Tell push subscribers a new frontend deployed (once per version — deduped in
# the DB). Never blocks serving: a failed announce is a slower banner, not an
# outage, and the client-side version poll still covers it.
python -m buildapp.app_update || echo "app-update announce failed (non-fatal)" >&2
exec skrift serve --host "${HOST:-0.0.0.0}" --port "${PORT:-8080}"
