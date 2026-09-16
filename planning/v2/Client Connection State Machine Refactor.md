# Client connection state-machine refactor

Status: all four planned stages implemented locally; rollout not performed.
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

## Stage 2: connection-attempt ownership

Replace `dialling` and `dialEra` with a per-device attempt controller. Its states
are `idle`, `connecting`, `succeeded`, `failed`, and `retired`. These describe
an attempt, not the device's ongoing availability: a successful attempt does
not imply the device can still answer later.

The controller owns deduplication, attempt identity, cancellation, retirement,
and resources acquired before handoff. Register the shared promise before
starting work so synchronous reentrant callers cannot create a second attempt.
Cancellation invalidates authority before running cleanup; resources arriving
after cancellation are disposed rather than adopted. A retired owner never
reopens, and account teardown retires pending attempts.

The connection coordinator must check attempt authority across every async
landing boundary, including the initial greeting. Only the current attempt may
publish a session, change failure state, or transfer resources to a live device
context. A stale success, failure, or cleanup cannot change a replacement
attempt, another device, or a newly initialized account.

Keep session cleanup before peer cleanup, preserve original failure causes,
and retain security-stop and explicit-retry behavior. Device availability,
long-lived peer ownership, and terminal-follow policy are still separate;
do not present this step as their completed migration.

## Stage 3: established connections and availability

One per-device lifecycle owns established session and peer resources, availability,
and the security refusal. The attempt controller remains a separate submachine:
starting a replacement attempt does not by itself make a usable connection
unavailable. An attempt identity and an established connection identity have
different lifetimes and must not substitute for each other.

Context transport and availability fields become read-only projections of that
owner. Existing adoption and offline helpers delegate commands to the same owner;
they must not maintain a second mutable availability record. Contexts still own
drafts, repositories, cache scope, and greeting capability selection. Unsupported
API versions remain distinct from transport failure in `canAnswer`.

Connection loss, presence-away, retry preparation, greeting failure, replacement,
security refusal, and retirement pass through lifecycle commands. Commit state
and invalidate the old connection's authority before invoking resource cleanup
or observer callbacks. Close the session before its peer, tolerate failed
disposers, and prevent old callbacks from affecting a replacement or another
device. A strict greeting failure marks the device unavailable before releasing
the greeting barrier and waking feed reads.

Account teardown cancels pending attempts and closes established resources.
Direct context retirement closes established resources and invalidates pending
results. Retired context getters stay retired even if the same device ID is
registered again. Preserve retry eligibility, outage timestamps,
sticky security refusals, and the existing rendezvous lease protocol.

## Stage 4: terminal-follow ownership

A terminal-follow controller owns an atomic device/context/session/carrier
identity and its pending transition. Replace the manager's separate follow
generation, pending-follow, carrier, and acknowledged-carrier variables. The
manager retains lazy socket creation, route/home policy, status subscriptions,
and the synchronous follow-request result used by routing.

Capture the chosen context and carrier when starting a transition. Verify that
capture at every async boundary and after synchronous effects; a late mint or
confirmation cannot be attributed to a replacement channel. Commit pending
ownership before invoking socket or lease callbacks, since those can reenter.
Retain the previous confirmed connection while minting a move to another device;
detach an unconfirmed superseded session so its late ping cannot report live.

Keep the terminal session's rendezvous lease through acknowledged confirmation,
then release it exactly once. Cancellation and failure release acquired leases;
late arrivals release their own leases without affecting the current follow.
No carrier means no acknowledgement: release an unadopted mint immediately
and mint afresh when a carrier arrives, rather than pinning an idle rendezvous.
Preserve fresh sessions on same-device
reconnect, isolated terminal-handshake failure, and explicit retry policy.
The controller never closes the shared app/terminal peer channels.

Application reset invalidates pending follows before teardown and prevents stale
results from attaching to a new account. Do not create a terminal socket merely
to reset it. TerminalSocket continues to own crypto/RPC, terminal registrations,
reattachment, and liveness; its transport machinery is not duplicated here.

## Boundaries retained

Keep account-wide presence, home-device selection, and view routing outside
the individual connection machines. Views request actions and observe state;
they do not manage transport resources.

## Verification gates

Write failing transition tests before implementation. For stage 1, cover
overlapping leases, duplicate release, force-close followed by retry, permanent
retirement, and teardown followed by re-registering the same device.

For stage 2, cover exact shared-promise identity, synchronous reentry,
immediate retry after settlement, prompt cancellation of never-settling work,
late resource disposal, and stale completion after replacement. Cleanup must
continue after a throwing disposer and invalidate authority before callbacks.
Exercise account reset and retirement during the initial greeting through the
real coordinator, retaining device-isolation and security-refusal regressions.

For stage 3, verify established ownership through replacement, channel loss,
presence-away, and retirement. Old connection callbacks must have no authority
over a new connection. Assert read-only context projection, preserved context
identity across reconnect, and retirement of captured contexts across same-ID
registration. Cover loss while another attempt is pending, reentrant and throwing
cleanup, security refusal across Retry/presence, and greeting failure before
feed wakeup. Audit production writers so no parallel transport/availability
record remains in the coordinator or contexts.

For stage 4, cover duplicate follow requests, superseded mint/confirmation,
same-device replacement, synchronous reentry, null-carrier adoption, failure
and retry, and reset followed by same-ID registration. Include context/carrier
changes without another follow call. Assert lease release exactly once and
that a stale completion cannot detach the current socket or release its lease.
Use real manager/socket tests for pending ping replacement, reset, isolated
confirmation failure, and continued connectivity beyond five seconds.

Integration tests must exercise the real connection coordinator: delayed initial
greeting, terminal confirmation overlapping ICE restart, and stale restart
callbacks after replacement. Retain the five-second terminal reconnect
regressions and device-isolation/security-stop tests.

Every stage must pass focused and full SPA tests, ESLint's complexity limit,
Semgrep, Gitleaks, and whitespace checks before commit. No new complexity-ratchet
exceptions or networking dependencies. This refactor does not authorize an
automatic production rollout; validate the first slice before expanding it.
