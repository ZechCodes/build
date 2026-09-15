# Strict P2P Transport Spec

Status: **binding** (2026-09-15). Supersedes the "stays on the relay" policy of
`WebRTC Transport Spec.md` §SPA carrier and migration policy (items 1, 2, 4, 5) and
`WebRTC Primitives.md` where they conflict. Everything else in those two documents
(envelope, carrier boundary, chunking, backpressure, session registry, bridge peer)
stays binding.

Decision taken 2026-09-15: Build is an open-source project that hosts as little as
possible. Conversations, commits, diffs and files are large; relaying any meaningful
share of them is the cost the hosted service must not carry. The hosted surface is
**authentication + rendezvous** and nothing else. Cloudflare TURN credential minting
stays as it is for now; its cost is revisited separately.

## Problem

Today the relay is the data plane whenever the peer path is not up: on boot until the
DataChannels open, whenever `openPeerLink` fails (the SPA "stays on the relay" and never
retries), and for the whole terminal stream until its channel arrives. The terminal
keeps a second relay socket open for the life of the tab. The relay also fans out
presence and transport keys that the api already owns. Each of those makes the hosted
relay a bandwidth product.

## Rules

Stated once each. Every stage's review checks them.

1. **The relay never carries application traffic.** The only frames a relay socket
   carries are the relay's own control frames, `session_init` / `session_accept`, and
   E2EE envelopes whose inner method is `rtc.*`. The bridge **enforces** this: a data
   frame arriving on a `Relay` carrier whose method is not `rtc.*` is refused with an
   error reply (`relay_is_not_a_data_plane`) and logged; it is never dispatched. The SPA
   never sends one. A relay frame is capped at **64 KiB**.
2. **The browser is live only over the DataChannels.** `App.call` and every terminal
   RPC ride the `app` / `term` channels. Nothing is dispatched to the user's surfaces
   before both channels are open; `session.hello` and `_reattachAll` run over the
   channels. There is no relay-carried "live the moment `session_accept` arrives".
3. **Fail closed.** If the peer connection cannot be established — no
   `RTCPeerConnection`, ICE servers unavailable, the offer refused, channels not open
   within the deadline, connection `failed` — the app enters an explicit **blocked**
   state (`App.blocked = {reason}`): a full-width banner naming the reason, a Retry
   action, composer and calls gated exactly as `App.offline` gates them. The relay
   session is closed. No silent fallback, no retry loop; Retry is a user action or a
   device switch.
4. **The relay socket is a rendezvous, not a connection.** The browser opens it to
   mint sessions and negotiate, and **closes it once both channels are open**. It
   reopens it on demand — a fresh gateway token, `session_init` re-attach with the same
   session id and key, then `rtc.offer {iceRestart:true}` — when the connection reports
   `failed`, and closes it again once `connected`. The bridge's device socket stays
   persistent (the bridge is behind NAT; the relay is how it is found).
5. **One relay socket, two sessions.** The app session and the terminal session are
   both minted over the same relay socket (the relay already routes N sessions per
   client socket). The terminal session never has its own socket; after mint it rides
   only the `term` channel. The two sessions keep separate keys, so the bridge's
   per-session state (terminal attachments, `term.ack` budgets) is unchanged.
6. **Presence is the api's.** The bridge posts a device-signed heartbeat to
   `POST /api/devices/heartbeat` every 30 s; the api derives `status` at read time
   (`online` iff `last_seen_at` is within 90 s, else `offline`; `pending` before
   approval as today). The relay no longer reports status, no longer accepts
   `transport_key`, and no longer pushes `device_key` / `device_online` /
   `device_offline`. The SPA reads presence from `GET /api/devices` and polls it (3 s
   on the gate as today, 15 s while the app is open, immediately on `visibilitychange`).
   The transport key the SPA seals to is the api-pinned one — it already is.
7. **Rendezvous is a seam.** In the SPA, everything that talks to the relay sits behind
   one `Rendezvous` interface: `{ open(deviceId) → {mint(sessionInitFor), signal, onPush,
   close}, ... }`, implemented today by `relayRendezvous`. In the bridge, `session_init`
   arrives through `FrameIntake::open` and `session_accept` is returned **over the
   carrier the `session_init` arrived on**, never over a relay-specific control
   channel; `CarrierKind` gains no variant now but nothing may match on `Relay` except
   rule 1's enforcement. This is what a future **direct network** mode (LAN, Tailscale)
   plugs into: a bridge-local listener becomes a second rendezvous implementation on
   both ends with no relay involved. It is not built in this plan.
8. **Prefer direct.** The bridge's ICE agent resolves browser mDNS host candidates
   (`MulticastDnsMode::QueryOnly`), gathers over UDP4/UDP6 on every non-loopback
   interface, and waits `BRIDGE_ICE_RELAY_MIN_WAIT_MS` (default 1500) before accepting
   a relay pair so a slower direct pair can win. `BRIDGE_ICE_POLICY=direct-only` strips
   TURN servers from the offered list and drops relay candidates (for LAN/Tailscale
   installs and the future direct mode). The SPA passes ICE servers through unchanged.

## What stays the same

- The E2EE envelope and `PROTOCOL_VERSION` 1. The cross-language interop test passes.
- Relay authentication: `/ws/device` Ed25519 challenge auth, 60 s re-validation,
  `/ws/client` gateway-token auth, same-owner scoping, session-id hijack guard.
- Relay `session_closed` to the device when a client socket dies: session bookkeeping
  the bridge uses to release that session's relay carrier. It is not a data path.
- The `SessionRegistry` teardown rule: a session ends when its last carrier ends or its
  client says so. Relay carrier loss under a live channel is already benign.
- The chunker, `DC_BUFFERED_HIGH`, the 8 MiB `MAX_REASSEMBLED_BYTES` on the channel
  (now its own constant, no longer "the relay's frame cap").
- Cloudflare TURN minting (`ice_servers.py`), TTL, and where the credentials travel.
- Device pairing, push notifications, transport telemetry: all already api-direct.

## Relay after this plan

| surface | keeps | drops |
|---|---|---|
| `/ws/device` | header auth, re-validation, `heartbeat` (liveness only), `session_init` in, `session_accept` / `e2ee_envelope` out, `session_closed` out | `transport_key`, `device_key` fan-out, `device_online` / `device_offline` fan-out, `/internal/devices/{id}/status` POST |
| `/ws/client` | `authenticate`, `session_init`, `e2ee_envelope` (signaling), `session_accept` / `e2ee_envelope` in | `device_key` snapshot, `device_online` / `device_offline` |
| limits | `MAX_WS_MESSAGE_BYTES = 64 KiB`, `MAX_OUTBOUND_QUEUE_BYTES = 1 MiB` | 8 MiB / 32 MiB |
| `/health` | unchanged | |

`serve_device` and `serve_client` come under the clippy threshold with the fan-out
gone; their two ratchet annotations are retired (`RATCHETED_FUNCTIONS` 28 → 26).

## SPA after this plan

- `core/rendezvous.js` — the `Rendezvous` interface and `relayRendezvous(...)`: one
  socket, `authenticate`, `mint(session_init)` → `session_accept` for any number of
  sessions, `signal(method, params)` over a session, `close()`. Replaces
  `relayLink.js`'s socket ownership; the backoff/re-attach logic moves here and is
  used only while a negotiation is in flight. Both `createRelayLink` ratchets retire
  (`RATCHETED_FUNCTIONS` 70 → 68) or the doc says why not.
- `core/sessionSwitch.js` — `wireFor(method)`: `rtc.*` → rendezvous (opening it if
  closed), everything else → the peer carrier or a **pending queue** while the upgrade
  is in flight; the queue is failed with `BlockedError` when the upgrade fails.
  `carrying = peer` — the relay is never the active carrier.
- `core/peerLink.js` — unchanged negotiation; gains `onConnected` (for closing the
  rendezvous) and `onFailed` (for reopening it); `restart` asks the rendezvous to open.
- `connection.js` — `openAppSession` becomes: open rendezvous → mint app + terminal
  sessions → `openPeerLink` → hello + terminal re-attach over the channels →
  `rendezvous.close()` → live. Any throw → `enterBlocked(reason)`. `goOffline` is
  reserved for a live peer connection that ends (channel close without a successful
  ICE restart); it drops to blocked with reason `connection lost`, and Retry runs the
  sequence again.
- `terminal/manager.js` / `terminal/session.js` — `TerminalSocket` no longer creates a
  relay link; it is constructed with the terminal session minted by `connection.js`
  and a `peer(carrier)` setter; `_reattachAll` runs on `onActive`.
- `devices.js` — presence from `GET /api/devices` only; `markDeviceOnline/Offline`
  deleted; a `watchPresence()` poller owned by `connection.js`.
- Blocked UI: `#blockbar` beside `#offbar`, text from `core/text.js`
  `blockedBannerText(reason)`, reasons: `no-webrtc`, `ice-servers`, `refused`,
  `timeout`, `failed`, `lost`.

## Bridge after this plan

- `presence.rs` (new): `PresenceReporter::start(api_url, identity)` — a 30 s task
  posting `{device_id, timestamp, signature_b64}` with challenge
  `heartbeat.{device_id}.{timestamp}`; mirrors `transport_report.rs`. One heartbeat is
  sent immediately on start and one `{online:false}`-equivalent is **not** sent on
  shutdown (status is derived from `last_seen_at`).
- `relay.rs` (client): no `transport_key` upload; `session_accept` goes back through
  the `CarrierHandle` the init arrived on (`FrameIntake::open` writes it to the
  carrier's outbound as a control frame variant, so a channel-borne init in a future
  direct mode needs no new path).
- `carrier/dispatch.rs`: rule 1 enforcement, keyed on `CarrierKind::Relay`.
- `rtc.rs`: `SettingEngine` per rule 8; `BRIDGE_ICE_POLICY`, `BRIDGE_ICE_RELAY_MIN_WAIT_MS`,
  `BRIDGE_ICE_INTERFACES` (optional allow-list, e.g. `tailscale0,eth0`).
- `transport_ledger.rs`: `FellBack` renamed `ChannelsLost` (meaning: the last channel
  closed while the session lives — an ICE restart is under way or the session is about
  to end). `transport_admin.py` bucket `RELAY_ONLY` → `NEVER_CONNECTED` with the same
  classification rule.

## api after this plan

- `POST /api/devices/heartbeat` — public, device-signed, replay-guarded (copy of the
  notify sequence: parse → approved+owned → challenge → `verify_registration` →
  `notify_timestamp_fresh` → `NotifyReplayGuard`). Sets `last_seen_at`.
- `device_summary` derives `status`; `Device.status` column keeps `pending` until
  approval and is otherwise ignored (no migration; a later cleanup may drop it).
- `POST /internal/devices/{id}/status` deleted.

## QA and deploy after this plan

- `deploy/compose.real.yml` QA runs a Rust peer client (`bridge/examples/qa_peer.rs`,
  built on `rtc/testing.rs`'s browser-side harness) that authenticates to the real
  relay with a gateway token, mints a session, negotiates DataChannels through the
  relay, and runs the e2e assertions over the channels. `web/qa.mjs` / `web/e2e.mjs`
  are retired from the compose profile and marked stale in `web/README.md` beside the
  three already listed.
- `deploy/k8s/relay.yaml`: memory limit 128Mi, the presence comment rewritten; the
  `Recreate` rationale becomes "in-flight negotiations, not held connections".
- README / HANDOFF / `deploy/README.md`: the "peers that cannot hole-punch simply keep
  working over the relay" sentence and the topology diagram are replaced.

## Non-goals

- Removing Cloudflare TURN or hosting a TURN of our own (revisited separately).
- Building the direct network rendezvous (rule 7 only reserves the seam).
- Multi-pod relay; the relay stays `replicas: 1`.
- Any envelope, key-exchange or forward-secrecy change.
- Mobile / non-browser clients.

## Open questions

1. Whether `session_closed` should also go (it is one relay message type; without it
   a vanished browser's relay carrier lingers on the bridge until the device socket
   drops).
2. Presence polling cadence while the app is open (15 s is a guess; the gate's 3 s is
   already in code).
