# Client connection state-machine refactor

Status: stage 1 implemented; subsequent stages planned.
Baseline: `39838a88` (2026-09-16). No production rollout in this stage.

## Objective

Replace distributed connection ownership with explicit state transitions without
changing the Strict P2P Transport Spec, wire protocol, device selection, or retry
policy. Preserve the existing transport implementations and public UI interfaces.
This is an incremental refactor, not a new networking stack.

## Stage 1: rendezvous ownership

Introduce `core/rendezvousLifecycle.js` as the sole owner of each device's
negotiation leases. Remove the parallel rendezvous-instance, reference-count,
and generation maps from `connection.js`.

Each device owner has explicit `idle`, `negotiating`, and `retired` states.

| Event | Transition and resource effect |
| --- | --- |
| Acquire | Idle becomes negotiating; another acquire joins the current negotiation. |
| Release | Remove only that lease; the last live lease closes the rendezvous and returns to idle. |
| Force close | Invalidate outstanding leases, close the rendezvous, return to idle for an explicit retry. |
| Retire | Invalidate leases, close the rendezvous, permanently reject work on that owner. |

Registry teardown retires captured owners. Registering the same device later
creates a new owner; old callbacks must not acquire or release its resources.
Lease release is idempotent. Stale negotiation callbacks must also be rejected
after a forced close, not just after device retirement.

The three users share this ownership mechanism:

- Initial app connection: retain the lease until the application greeting is
  acknowledged over the app DataChannel; release on success or failure.
- ICE restart: retain a separate lease from restart signaling through confirmed
  connectivity. An old peer's callbacks cannot affect a replacement connection.
- Terminal session: retain its lease through the terminal channel's acknowledged
  ping, including when no terminal panes are mounted.

This stage is deliberately not the complete device state machine. Existing
device availability and terminal-follow policy remain in place.

## Subsequent stages

1. A per-device lifecycle controller owns connection attempts, session and peer
   resources, cancellation, security refusals, and named lifecycle events.
2. Device-context availability becomes a projection of that controller. Remove
   competing offline/blocked writers only when the controller is authoritative;
   do not maintain a shadow state machine beside the old booleans.
3. A terminal-follow controller owns an atomic device/session/carrier identity
   and its pending transition, replacing separate follow-generation and carrier
   tracking. Preserve isolated terminal-handshake failure and stale-reply guards.
4. Keep account-wide presence, home-device selection, and view routing outside
   the individual connection machines. Views request actions and observe state;
   they do not manage transport resources.

## Verification gates

Write failing transition tests before implementation. For stage 1, cover
overlapping leases, duplicate release, force-close followed by retry, permanent
retirement, and teardown followed by re-registering the same device.

Integration tests must exercise the real connection coordinator: delayed initial
greeting, terminal confirmation overlapping ICE restart, and stale restart
callbacks after replacement. Retain the five-second terminal reconnect
regressions and device-isolation/security-stop tests.

Every stage must pass focused and full SPA tests, ESLint's complexity limit,
Semgrep, Gitleaks, and whitespace checks before commit. No new complexity-ratchet
exceptions or networking dependencies. This refactor does not authorize an
automatic production rollout; validate the first slice before expanding it.
