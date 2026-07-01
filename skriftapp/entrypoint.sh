#!/bin/sh
# Container entrypoint: apply migrations (Skrift core + buildapp), then serve.
set -eu

skrift db upgrade heads
exec skrift serve --host "${HOST:-0.0.0.0}" --port "${PORT:-8080}"
