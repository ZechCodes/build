#!/usr/bin/env bash
# #166: the liveness stack under compose project drain166, a coturn on its
# network, and the soak. Run under the docker group (`echo … | newgrp docker`).
#
#   stack.sh up            build app/relay/bridge/qa, start, coturn, pair
#   stack.sh rebuild       rebuild the bridge only, recreate it, pair the new device
#   stack.sh soak NAME     one idle soak (env passes through: RELAY_BOTH_ENDS,
#                          ICE_TRANSPORT_POLICY, SOAK_MS, PING_EVERY_MS)
#   stack.sh down
set -euo pipefail
cd "$(dirname "$0")/../../.."   # the repo root
project=drain166
turn=${project}-turn
network=${project}_default
logs=${LOGS:-bridge/experiments/166/runs}
mkdir -p "$logs"
compose=(docker compose -p "$project" -f deploy/compose.real.yml -f deploy/compose.liveness.yml -f bridge/experiments/166/compose.drain166.yml)
say() { printf '%s drain166: %s\n' "$(date -u +%H:%M:%S)" "$*"; }

coturn() {
  docker rm -f "$turn" >/dev/null 2>&1 || true
  docker run -d --name "$turn" --network "$network" coturn/coturn:latest \
    -n --listening-port=3478 --fingerprint --lt-cred-mech --user=build:soak \
    --realm=build.test --no-tls --no-cli --log-file=stdout >/dev/null
}

counters() { # packets in/out of the bridge and of coturn, from their netns
  local b t
  b=$(docker exec "${project}-bridge-1" awk '/eth0/{print $3, $11}' /proc/net/dev)
  t=$(docker exec "$turn" sh -c "awk '/eth0/{print \$3, \$11}' /proc/net/dev" 2>/dev/null || echo "? ?")
  echo "bridge_rx_tx=$b turn_rx_tx=$t"
}

case "${1:-}" in
  up)
    "${compose[@]}" --profile qa build app relay bridge qa
    "${compose[@]}" up -d app relay
    for _ in $(seq 1 120); do
      code=$(curl -s -o /dev/null -w '%{http_code}' http://localhost:8166/auth/dummy/login || true)
      case "$code" in 2* | 3* | 4*) break ;; esac
      sleep 1
    done
    "${compose[@]}" up -d bridge
    coturn
    for attempt in $(seq 1 10); do
      "${compose[@]}" --profile qa run --rm --no-deps -T qa node pair.mjs </dev/null && break
      [ "$attempt" = 10 ] && { say "pairing failed"; exit 1; }
      sleep 3
    done
    ;;
  rebuild) # a recreated bridge is a new device; a fresh stack keeps the pairing simple
    docker rm -f "$turn" >/dev/null 2>&1 || true
    "${compose[@]}" --profile qa down -v
    exec "$0" up
    ;;
  soak)
    name=$2
    turn_ip=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$network\").IPAddress}}" "$turn")
    out="$logs/$name.log"
    {
      say "run $name  $(docker exec "${project}-bridge-1" sh -c 'sha256sum /usr/local/bin/build-bridge' | cut -c1-16)"
      say "loadavg before: $(cat /proc/loadavg)"
      say "counters before: $(counters)"
    } >"$out"
    # the soak, plus one line with every round trip (a mounted copy; web/ is untouched)
    sed 's|^  console.log(`RESULT |  console.log(`rtts         ${JSON.stringify(rtts.map((v) => +v.toFixed(2)))}`);\n&|' \
      web/liveness-soak.mjs >bridge/experiments/166/.soak-rtts.mjs
    grep -q '^  console.log(`rtts ' bridge/experiments/166/.soak-rtts.mjs
    "${compose[@]}" --profile qa run --rm --no-deps -T \
      -v "$PWD/bridge/experiments/166/.soak-rtts.mjs:/app/soak-rtts.mjs:ro" \
      -e TURN_HOST="$turn_ip" -e SOAK_MS="${SOAK_MS:-500000}" -e PING_EVERY_MS="${PING_EVERY_MS:-2000}" \
      -e RELAY_BOTH_ENDS="${RELAY_BOTH_ENDS:-1}" -e ICE_TRANSPORT_POLICY="${ICE_TRANSPORT_POLICY:-relay}" \
      -e PREFER_DEVICE_ID="${PREFER_DEVICE_ID:-}" \
      qa node soak-rtts.mjs </dev/null >>"$out" 2>&1 || echo "soak exit $?" >>"$out"
    {
      say "counters after: $(counters)"
      say "loadavg after: $(cat /proc/loadavg)"
    } >>"$out"
    sed -n '/liveness summary/,$p' "$out"
    ;;
  down)
    docker rm -f "$turn" >/dev/null 2>&1 || true
    "${compose[@]}" --profile qa down -v
    ;;
  *) echo "usage: $0 up|rebuild|soak NAME|down" >&2; exit 64 ;;
esac
