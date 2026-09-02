#!/usr/bin/env bash
# Create the Build k8s Secrets in namespace 8ly — idempotently. Existing secrets
# are NEVER regenerated (rotating the postgres password under a live volume
# would lock the app out; rotating VAPID keys would orphan every push
# subscription), so re-running is always safe. Missing keys on an existing
# secret are added without touching the ones already there.
#
#   deploy/k8s/bootstrap-secrets.sh [kubectl-context]
#
# Creates:
#   build-postgres  POSTGRES_PASSWORD
#   build-app       SECRET_KEY, INTERNAL_API_SECRET, DATABASE_URL,
#                   VAPID_PRIVATE_KEY, VAPID_PUBLIC_KEY, VAPID_SUBJECT (web push),
#                   SMTP_USERNAME, SMTP_PASSWORD, SMTP_FROM_ADDRESS,
#                   WAITLIST_NOTIFY_ADDRESS (outbound email)
#   build-relay     RELAY_INTERNAL_SECRET (same value as INTERNAL_API_SECRET)
#
# The four email keys are credentials this script cannot invent: export
# SMTP_USERNAME, SMTP_PASSWORD, SMTP_FROM_ADDRESS and WAITLIST_NOTIFY_ADDRESS in
# the environment before running, or the SMTP step aborts.
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

require_env() { [[ -n "${!1:-}" ]] || { echo "bootstrap-secrets: $1 must be set in the environment (never generated)" >&2; exit 1; }; }

b64url() { base64 | tr -d '=\n' | tr '/+' '_-'; }

# Generate a VAPID (P-256) keypair into VAPID_PRIVATE_KEY / VAPID_PUBLIC_KEY:
# the raw 32-byte private scalar and the 65-byte uncompressed public point,
# both unpadded base64url — the formats pywebpush and PushManager.subscribe
# expect. (SEC1 DER for prime256v1 carries the scalar at bytes 8–39.)
generate_vapid_keys() {
  local pem
  pem="$(mktemp)"
  openssl ecparam -name prime256v1 -genkey -noout -out "$pem"
  VAPID_PUBLIC_KEY="$(openssl ec -in "$pem" -pubout -outform DER 2>/dev/null | tail -c 65 | b64url)"
  VAPID_PRIVATE_KEY="$(openssl ec -in "$pem" -outform DER 2>/dev/null | tail -c +8 | head -c 32 | b64url)"
  rm -f "$pem"
}

VAPID_SUBJECT="mailto:ops@getbuild.ing"

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
  generate_vapid_keys
  kc create secret generic build-app \
    --from-literal=SECRET_KEY="$(random_secret)" \
    --from-literal=INTERNAL_API_SECRET="$(random_secret)" \
    --from-literal=DATABASE_URL="postgresql+asyncpg://build:${POSTGRES_PASSWORD}@build-postgres:5432/build" \
    --from-literal=VAPID_PRIVATE_KEY="$VAPID_PRIVATE_KEY" \
    --from-literal=VAPID_PUBLIC_KEY="$VAPID_PUBLIC_KEY" \
    --from-literal=VAPID_SUBJECT="$VAPID_SUBJECT"
  echo "secret build-app: created"
fi
INTERNAL_API_SECRET="$(secret_value build-app INTERNAL_API_SECRET)"

# Add-if-missing: a build-app secret created before web push landed gets VAPID
# keys patched in; existing keys are never regenerated (rotation would orphan
# every stored push subscription).
if [[ -z "$(kc get secret build-app -o 'jsonpath={.data.VAPID_PUBLIC_KEY}')" ]]; then
  generate_vapid_keys
  kc patch secret build-app --type merge -p "{\"stringData\":{
    \"VAPID_PRIVATE_KEY\":\"$VAPID_PRIVATE_KEY\",
    \"VAPID_PUBLIC_KEY\":\"$VAPID_PUBLIC_KEY\",
    \"VAPID_SUBJECT\":\"$VAPID_SUBJECT\"}}"
  echo "secret build-app: VAPID keys added"
fi

# --- build-app smtp (add-if-missing) -----------------------------------------
# The FastMail credentials and the owner notification address the waitlist mail
# needs. They are never generated — the operator exports them before running.
if [[ -z "$(kc get secret build-app -o 'jsonpath={.data.SMTP_USERNAME}')" ]]; then
  require_env SMTP_USERNAME
  require_env SMTP_PASSWORD
  require_env SMTP_FROM_ADDRESS
  require_env WAITLIST_NOTIFY_ADDRESS
  kc patch secret build-app --type merge -p "{\"stringData\":{
    \"SMTP_USERNAME\":\"${SMTP_USERNAME}\",
    \"SMTP_PASSWORD\":\"${SMTP_PASSWORD}\",
    \"SMTP_FROM_ADDRESS\":\"${SMTP_FROM_ADDRESS}\",
    \"WAITLIST_NOTIFY_ADDRESS\":\"${WAITLIST_NOTIFY_ADDRESS}\"}}"
  echo "secret build-app: SMTP keys added"
fi

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
