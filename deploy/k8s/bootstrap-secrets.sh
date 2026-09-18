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
#                   WAITLIST_NOTIFY_ADDRESS (outbound email),
#                   CF_TURN_KEY_ID, CF_TURN_KEY_API_TOKEN (Cloudflare TURN)
#   build-relay     RELAY_INTERNAL_SECRET (same value as INTERNAL_API_SECRET)
#
# The email keys are credentials this script cannot invent: export
# SMTP_USERNAME, SMTP_PASSWORD and SMTP_FROM_ADDRESS (and optionally
# WAITLIST_NOTIFY_ADDRESS) in the environment before running, or the SMTP step
# aborts.
#
# The Cloudflare TURN key is minted in the Cloudflare dashboard, so it cannot be
# invented either — but the app runs without it (the ICE-servers route answers a
# STUN-only list), so exporting CF_TURN_KEY_ID and CF_TURN_KEY_API_TOKEN is
# optional and their absence is reported, not fatal. Public release downloads
# need no GitHub credential on the website.
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

# Secret values go into `.data` as base64 rather than into `.stringData` raw:
# base64 is always JSON-safe, so a quote or a backslash in a password or a
# display-name from address cannot break (or extend) the merge patch.
b64_value() { printf %s "$1" | base64 | tr -d '\n'; }

app_key_missing() { [[ -z "$(kc get secret build-app -o "jsonpath={.data.$1}")" ]]; }

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
# The FastMail credentials the waitlist mail needs, plus the owner notification
# address (empty disables the notification). Never generated — the operator
# exports them before running.
if app_key_missing SMTP_USERNAME || app_key_missing SMTP_PASSWORD \
  || app_key_missing SMTP_FROM_ADDRESS || app_key_missing WAITLIST_NOTIFY_ADDRESS; then
  require_env SMTP_USERNAME
  require_env SMTP_PASSWORD
  require_env SMTP_FROM_ADDRESS
  kc patch secret build-app --type merge -p "{\"data\":{
    \"SMTP_USERNAME\":\"$(b64_value "$SMTP_USERNAME")\",
    \"SMTP_PASSWORD\":\"$(b64_value "$SMTP_PASSWORD")\",
    \"SMTP_FROM_ADDRESS\":\"$(b64_value "$SMTP_FROM_ADDRESS")\",
    \"WAITLIST_NOTIFY_ADDRESS\":\"$(b64_value "${WAITLIST_NOTIFY_ADDRESS:-}")\"}}"
  echo "secret build-app: SMTP keys added"
fi

# --- build-app cloudflare turn (add-if-missing) -------------------------------
# The TURN key the ICE-servers route mints per-user credentials from. Never
# generated: it comes from the Cloudflare dashboard. Without it the route answers
# a STUN-only list and direct paths still carry every peer that can hole-punch,
# so a bootstrap with no key is a supported deployment, not a failure.
if app_key_missing CF_TURN_KEY_ID || app_key_missing CF_TURN_KEY_API_TOKEN; then
  if [[ -n "${CF_TURN_KEY_ID:-}" && -n "${CF_TURN_KEY_API_TOKEN:-}" ]]; then
    kc patch secret build-app --type merge -p "{\"data\":{
      \"CF_TURN_KEY_ID\":\"$(b64_value "$CF_TURN_KEY_ID")\",
      \"CF_TURN_KEY_API_TOKEN\":\"$(b64_value "$CF_TURN_KEY_API_TOKEN")\"}}"
    echo "secret build-app: Cloudflare TURN keys added"
  else
    echo "secret build-app: no CF_TURN_KEY_ID/CF_TURN_KEY_API_TOKEN in the environment, ICE stays STUN-only"
  fi
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
