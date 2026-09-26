#!/bin/sh
# Container entrypoint: apply migrations (Skrift core + buildapp), then serve.
#
# Production sets BUILD_MIGRATE_ON_START=0 (deploy/k8s/app.yaml): there the
# build-app-migrate Job (deploy/k8s/migrate.yaml) runs `skrift db upgrade heads`
# once per deploy, before the rollout, so two pods overlapping in a rolling
# update never both write the schema. Anywhere else (a plain `podman run`), the
# default keeps migrate-then-serve.
set -eu

if [ "${BUILD_MIGRATE_ON_START:-1}" != "0" ]; then
  skrift db upgrade heads
fi
# Tell push subscribers a new frontend deployed (once per version — deduped in
# the DB). Never blocks serving: a failed announce is a slower banner, not an
# outage, and the client-side version poll still covers it.
#
# Not until 90 s after start. In a rolling deploy this pod starts beside the old
# one, which serves the old version.json until this pod is ready and Traefik has
# dropped the old one: up to 60 s of boot (the startup probe's allowance), a 5 s
# readiness period, and Traefik's ~2 s reload. A client told to re-check any
# sooner could ask the old pod and see no update. A pod that dies first (a
# failed rollout) announces nothing, which is right: its version never served.
(
  sleep 90
  python -m buildapp.app_update || echo "app-update announce failed (non-fatal)" >&2
) &
exec skrift serve --host "${HOST:-0.0.0.0}" --port "${PORT:-8080}"
