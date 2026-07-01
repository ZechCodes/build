#!/usr/bin/env bash
# Create the Build k8s Secrets in namespace 8ly — idempotently. Existing secrets
# are NEVER regenerated (rotating the postgres password under a live volume
# would lock the app out), so re-running is always safe.
#
#   deploy/k8s/bootstrap-secrets.sh [kubectl-context]
#
# Creates:
#   build-postgres  POSTGRES_PASSWORD
#   build-app       SECRET_KEY, INTERNAL_API_SECRET, DATABASE_URL
#   build-relay     RELAY_INTERNAL_SECRET (same value as INTERNAL_API_SECRET)
set -euo pipefail

NAMESPACE=8ly
KUBECTL=(kubectl)
if [[ $# -ge 1 ]]; then
  KUBECTL=(kubectl --context "$1")
fi

kc() { "${KUBECTL[@]}" -n "$NAMESPACE" "$@"; }

secret_exists() { kc get secret "$1" >/dev/null 2>&1; }

secret_value() { # secret_value <secret> <key>
  kc get secret "$1" -o "jsonpath={.data.$2}" | base64 -d
}

random_secret() { openssl rand -hex 32; }

"${KUBECTL[@]}" get namespace "$NAMESPACE" >/dev/null 2>&1 \
  || "${KUBECTL[@]}" create namespace "$NAMESPACE"

# --- build-postgres ----------------------------------------------------------
if secret_exists build-postgres; then
  echo "secret build-postgres: exists, leaving untouched"
else
  kc create secret generic build-postgres \
    --from-literal=POSTGRES_PASSWORD="$(random_secret)"
  echo "secret build-postgres: created"
fi
POSTGRES_PASSWORD="$(secret_value build-postgres POSTGRES_PASSWORD)"

# --- build-app ----------------------------------------------------------------
if secret_exists build-app; then
  echo "secret build-app: exists, leaving untouched"
else
  kc create secret generic build-app \
    --from-literal=SECRET_KEY="$(random_secret)" \
    --from-literal=INTERNAL_API_SECRET="$(random_secret)" \
    --from-literal=DATABASE_URL="postgresql+asyncpg://build:${POSTGRES_PASSWORD}@build-postgres:5432/build"
  echo "secret build-app: created"
fi
INTERNAL_API_SECRET="$(secret_value build-app INTERNAL_API_SECRET)"

# --- build-relay ---------------------------------------------------------------
# The relay authenticates to the app's /internal/* routes with the same secret.
if secret_exists build-relay; then
  echo "secret build-relay: exists, leaving untouched"
else
  kc create secret generic build-relay \
    --from-literal=RELAY_INTERNAL_SECRET="$INTERNAL_API_SECRET"
  echo "secret build-relay: created"
fi

echo "done."
