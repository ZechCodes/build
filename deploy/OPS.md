# Ops checklist

Recurring checks on the live stack (namespace `8ly`). One-time deploy and
cutover steps live in [`k8s/CUTOVER.md`](k8s/CUTOVER.md); how to run the stack
locally is in [`README.md`](README.md).

## Monthly — Cloudflare TURN usage

Cloudflare TURN is free to 1000 GB of egress to clients per month and $0.05
per GB after that. Only relayed sessions bill: a peer connection that
settles on a direct (`host`, `srflx` or `prflx`) candidate pair costs
nothing, and a session that never upgrades off the relay costs nothing here
either.

1. Read the month's TURN egress in the Cloudflare dashboard (Realtime → TURN),
   or query the same numbers from Cloudflare's GraphQL analytics api. Traffic
   shows up within 30 seconds, so the figure is current, not a billing-cycle
   estimate.
2. Compare it against the 1000 GB included. Above it, budget $0.05 per further
   GB.
3. If egress is climbing, ask how many sessions are actually being relayed. The
   bridge writes one line to stderr per session, the moment its peer connection
   first carries:

   ```
   rtc: session <session_id> carrying over relay candidates
   ```

   Count the billed ones with:

   ```bash
   grep 'carrying over relay' bridge.err.log | wc -l
   ```

   `host`, `srflx` and `prflx` are the direct, free paths; `relay` is the
   billed one; `unknown` means the bridge found no nominated pair in its stats
   report — count it as unbilled, but a rise in `unknown` is a bridge bug, not
   TURN usage. A rise in `relay` lines without a rise in users means more
   clients are failing to hole-punch, not that each client is moving more
   bytes.

## Monthly — the TURN key still works

Egress falling to zero while relay traffic holds is not everyone hole-punching:
it is what a dead key looks like. Cloudflare rejecting `CF_TURN_KEY_ID` /
`CF_TURN_KEY_API_TOKEN` makes `POST /api/rtc/ice-servers` answer 502, which
makes the browser's upgrade fail and leaves every session on the relay —
working, slower, and silent about why. Zero egress plus doubled relay load is
that failure, not good news.

1. Ask the api for a list with the key the pod actually holds:

   ```bash
   kubectl --context do-nyc1-production-hosting -n 8ly exec deploy/build-app -- \
     python -c "import os; from buildapp.ice_servers import ice_servers; \
       print(ice_servers(os.environ.get('CF_TURN_KEY_ID', ''), os.environ.get('CF_TURN_KEY_API_TOKEN', '')))"
   ```

   A TURN entry with `username` / `credential` is a live key. An
   `IceServersUnavailable` traceback naming Cloudflare's status is a dead one.
   The bare `stun:stun.cloudflare.com:3478` list means no key is configured at
   all, which is a supported deployment (see [`README.md`](README.md)) but not
   what a production pod should print.
2. Replace a dead key. `k8s/bootstrap-secrets.sh` only adds the keys when they
   are missing, so it will not overwrite one, and the Deployment reads the
   Secret at start:

   ```bash
   kubectl --context do-nyc1-production-hosting -n 8ly patch secret build-app --type merge \
     -p '{"data":{"CF_TURN_KEY_ID":"<base64>","CF_TURN_KEY_API_TOKEN":"<base64>"}}'
   kubectl --context do-nyc1-production-hosting -n 8ly rollout restart deploy/build-app
   ```

   Then re-run step 1 against the new pod.
