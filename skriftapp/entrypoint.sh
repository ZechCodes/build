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
exec skrift serve --host "${HOST:-0.0.0.0}" --port "${PORT:-8080}"
