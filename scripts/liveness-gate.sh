#!/usr/bin/env bash
# The TURN soak as a gate (#131 §5).
#
# Brings up the liveness stack (deploy/compose.liveness.yml over
# deploy/compose.real.yml: the bridge squeezed to LIVENESS_BRIDGE_WORKERS tokio
# workers and LIVENESS_BRIDGE_CPUS cpus), a coturn on its network, and pairs the
# bridge. Then, at the same time:
#
#   - web/liveness-soak.mjs holds one TURN-only session for SOAK_MS (ten
#     minutes) with busy loops spawned through term.create and an issues.list
#     hammer, and fails on any drop, any timeout, or any ping at 500 ms or over;
#   - web/ice-restart-check.mjs, RESTART_AT_S into the soak, runs the real app
#     in Chromium over the same TURN and puts it through an ICE restart, which
#     fails unless it lands within 15 s on the relayed path and holds.
#
# Exits non-zero if either failed, and takes everything down whatever happened.
# Needs docker (with compose) and nothing else: the soak runs in the stack's qa
# image and the browser in mcr.microsoft.com/playwright, on the host's network
# so it reaches the stack's published ports and coturn's address.
#
#   scripts/liveness-gate.sh
#   SOAK_MS=120000 RESTART_AT_S=45 scripts/liveness-gate.sh   # a short run
#
# Knobs: SOAK_MS (600000) RESTART_AT_S (240) LOAD_TERM_THREADS (4)
#   LIVENESS_PROJECT (liveness-gate) LIVENESS_LOGS (a temp dir; kept)
#   PLAYWRIGHT_IMAGE (mcr.microsoft.com/playwright:v1.63.0-noble)
#   LIVENESS_BRIDGE_WORKERS LIVENESS_BRIDGE_CPUS (compose.liveness.yml's)
set -euo pipefail
cd "$(dirname "$0")/.."

project=${LIVENESS_PROJECT:-liveness-gate}
soak_ms=${SOAK_MS:-600000}
restart_at_s=${RESTART_AT_S:-240}
term_threads=${LOAD_TERM_THREADS:-4}
playwright_image=${PLAYWRIGHT_IMAGE:-mcr.microsoft.com/playwright:v1.63.0-noble}
playwright_version=${playwright_image##*:v}
playwright_version=${playwright_version%%-*}
logs=${LIVENESS_LOGS:-$(mktemp -d -t liveness-gate.XXXXXX)}
mkdir -p "$logs"
turn=${project}-turn
network=${project}_default
compose=(docker compose -p "$project" -f deploy/compose.real.yml -f deploy/compose.liveness.yml)

say() { printf '%s liveness-gate: %s\n' "$(date -u +%H:%M:%S)" "$*"; }

teardown() {
  "${compose[@]}" logs --no-color --tail 400 bridge >"$logs/bridge.log" 2>&1 || true
  docker rm -f "$turn" >/dev/null 2>&1 || true
  "${compose[@]}" --profile qa down -v >/dev/null 2>&1 || true
  say "logs in $logs"
}
trap teardown EXIT

say "building the stack"
"${compose[@]}" --profile qa build app relay bridge qa >"$logs/build.log" 2>&1

# The app first, and answering, before the bridge registers with it: a bridge
# that registers against an app still migrating its database is a device the
# app never heard of.
say "starting the app and the relay"
"${compose[@]}" up -d app relay >>"$logs/build.log" 2>&1
for _ in $(seq 1 120); do
  code=$(curl -s -o /dev/null -w '%{http_code}' http://localhost:8128/auth/dummy/login || true)
  case "$code" in 2* | 3* | 4*) break ;; esac
  sleep 1
done
say "starting the bridge"
"${compose[@]}" up -d bridge >>"$logs/build.log" 2>&1

say "starting coturn on $network"
docker rm -f "$turn" >/dev/null 2>&1 || true
docker run -d --name "$turn" --network "$network" coturn/coturn:latest \
  -n --listening-port=3478 --fingerprint --lt-cred-mech --user=build:soak \
  --realm=build.test --no-tls --no-cli --log-file=stdout >/dev/null
turn_ip=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$network\").IPAddress}}" "$turn")
say "coturn at $turn_ip"

say "pairing"
for attempt in $(seq 1 10); do
  if "${compose[@]}" --profile qa run --rm --no-deps -T qa node pair.mjs </dev/null >>"$logs/pair.log" 2>&1; then
    break
  fi
  [ "$attempt" = 10 ] && { say "pairing failed"; tail -20 "$logs/pair.log"; exit 1; }
  sleep 3
done

say "soaking $((soak_ms / 1000)) s; the ICE restart comes at ${restart_at_s} s"
"${compose[@]}" --profile qa run --rm --no-deps -T \
  -e TURN_HOST="$turn_ip" -e SOAK_MS="$soak_ms" -e LOAD_TERM_THREADS="$term_threads" \
  -e HAMMER_ISSUES=1 -e PROBE_STATS=1 \
  qa node liveness-soak.mjs </dev/null >"$logs/soak.log" 2>&1 &
soak=$!

sleep "$restart_at_s"
restart_exit=0
docker run --rm --network host --ipc=host \
  -v "$PWD/web:/check:ro" -e APP=http://localhost:8128 -e TURN_HOST="$turn_ip" \
  "$playwright_image" bash -c \
  "cp /check/ice-restart-check.mjs /tmp/ && cd /tmp && npm install --silent --no-audit --no-fund playwright@$playwright_version >/dev/null && node ice-restart-check.mjs" \
  >"$logs/ice-restart.log" 2>&1 || restart_exit=$?

soak_exit=0
wait "$soak" || soak_exit=$?

say "the soak:"
sed -n '/liveness summary/,$p' "$logs/soak.log"
say "the ICE restart:"
grep -E "restart|RESULT|never connected|pageerror|ice-restart-check:" "$logs/ice-restart.log" || tail -20 "$logs/ice-restart.log"
say "soak exit $soak_exit, ICE restart exit $restart_exit"
[ "$soak_exit" = 0 ] && [ "$restart_exit" = 0 ]
