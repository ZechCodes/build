# Ops checklist

Recurring checks on the live stack (namespace `8ly`). One-time deploy and
cutover steps live in [`k8s/CUTOVER.md`](k8s/CUTOVER.md); how to run the stack
locally is in [`README.md`](README.md).

## Monthly — Cloudflare TURN usage

Cloudflare TURN is free to 1000 GB of egress to clients per month and $0.05 per
GB after that. Only relayed sessions bill: a peer connection that settles on a
direct (`host` or `srflx`) candidate pair costs nothing, and a session that
never upgrades off the relay costs nothing here either.

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

   `host` and `srflx` are the direct, free paths; `relay` is the billed one. A
   rise in `relay` lines without a rise in users means more clients are failing
   to hole-punch, not that each client is moving more bytes.
