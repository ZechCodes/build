# WebRTC Transport Spec

Status: **draft for review** (2026-09-02). Branch: `build/webrtc`.

Decision taken 2026-09-02: Cloudflare TURN plus direct WebRTC DataChannels between
browser and bridge. Direct (host/srflx) ICE candidates are allowed, so the two peers may
learn each other's IP. The WebSocket relay stays as transport #1 for authentication,
presence, signaling, and fallback. The Cloudflare Realtime SFU is not used.

## Problem

Every byte between a browser and a bridge rides one path today: browser → relay pod →
bridge, over two WebSockets per tab. That path has two costs that the E2EE design does
not need:

- **Latency and throughput are bounded by a single pod.** The relay is one in-memory
  broker (`deploy/k8s/relay.yaml` pins `replicas: 1` + `Recreate` because per-pod
  connection state cannot be split). Terminal bursts and multi-megabyte diffs all pass
  through it, and the roadmap's "connection tier you redeploy often must not hold the
  connections you can't afford to drop" principle is violated by the relay itself.
- **The relay carries application traffic it has no role in.** The crypto layer
  (`bridge/src/transport.rs`, `@build/secure-transport`) never touches a socket, and
  the threat model in `build-secure-transport/E2E-MVP.md` already treats the relay as a
  non-party that can only drop, delay, or replay. What the relay uniquely provides is
  auth, per-user device scoping, presence fan-out, and rendezvous. None of those need
  to see the terminal bytes.

A second transport lets the relay keep the control-plane job and hands the data plane to
a direct peer path, with Cloudflare TURN covering the peers that cannot hole-punch.

## What stays the same

These are invariants, not aspirations. Each stage's review checks them.

1. **The E2EE envelope is unchanged.** Outer `{version, session_id, route_to, nonce,
   ciphertext}` and inner `{session_id, message_id, frame_type, sender, created_at,
   payload}` are byte-identical on both carriers. `PROTOCOL_VERSION` stays 1. The
   cross-language interop test (`bridge/tests/interop_python.rs`) keeps passing.
2. **One E2EE session, two carriers.** A session (`session_id` → session key) is minted
   once over the relay via `session_init` / `session_accept` and is then reachable over
   either carrier. No second handshake, no second key.
3. **The relay protocol is unchanged.** No new relay message types. The relay never sees
   an SDP, an ICE candidate, or a TURN credential.
4. **The application RPC surface is unchanged.** The 61 `App.call` sites, the terminal
   panes, `dispatch_frame`, `term.ack` flow control, and snapshot + cursor resync all
   work the same on both carriers.
5. **Fail fast.** A carrier that cannot be established is reported and not retried in a
   loop; the relay path is the fallback, not a retry target.

## Design

### Carrier boundary (bridge)

Today `SessionSender` (`bridge/src/relay.rs`) owns an
`mpsc::UnboundedSender<tungstenite::Message>` and formats the relay wire wrapper inside
`push`. That makes every holder (`TermScreen.attached`, `ChangeBus.subscribers`)
transitively typed on the WebSocket.

Replace it with a carrier-neutral outbound:

```rust
/// One encrypted frame bound for one client session. The carrier decides the wire wrapper.
/// The session it is bound for is the envelope's own, stamped by `encrypt_frame`.
pub struct OutboundEnvelope(Envelope);

pub struct SessionSender {
    session_id: String,
    session_key: String,
    out: mpsc::UnboundedSender<OutboundEnvelope>,
    still_open: Arc<AtomicBool>,
}
```

`still_open` is the opening this sender was built for, cleared by the registry when that
opening ends. It is what "nothing runs for a session after its close" rests on: a frame
admitted on one carrier and dispatched after another carrier ended the session reads it
and never runs.

- The relay writer task wraps each `OutboundEnvelope` as
  `{"type":"e2ee_envelope","session_id":…,"envelope":…}`.
- The DataChannel writer task sends the envelope JSON directly (chunked, see below).
- `route_to` keeps its current values (`session:<id>` from the device, `device:<id>` from
  the browser). On the DataChannel it is ignored by the receiver, as it is today on the
  bridge side (`transport.rs` only checks it is non-empty).

### Shared session registry (bridge)

The `session_id → session_key` map currently lives inside the relay read loop and dies
with the socket. Move it to a `SessionRegistry` owned by `main.rs` and shared by both
carriers. The relay loop inserts on `session_accept`; the DataChannel carrier looks keys
up when a frame arrives with a known `session_id`. A frame for an unknown session is
dropped without a trace: a paired browser must not be able to drive the device's stderr
from the frame path. The one refusal the device logs is the `session_init` below that
contests a live session under a foreign key, one line each, because that is a client
contesting a session someone else holds. `end_session` (relay `session_closed`, client
`close` frame, or DataChannel close of the session's last carrier) removes the key.

The relay still has last-writer-wins semantics per device connection. When the bridge's
relay socket reconnects, sessions minted on the previous socket are still valid on the
DataChannel. This is the first time a session outlives a relay socket, and the SPA
policy below depends on it. Two rules make that hold:

- **A session ends when its last carrier ends, not when the relay says `session_closed`.**
  `session_closed` from the relay (or a relay socket loss) detaches the relay carrier from
  the session. If a DataChannel carrier for that session is live, the session and its key
  stay in the registry. The client `close` frame still ends the session outright.
- **A relay carrier re-attaches by repeating `session_init` with the existing session id
  and key.** The browser keeps its session id and session key across relay socket
  reconnects. When the new socket is authenticated it sends `session_init` for that same
  session. The bridge treats a `session_init` for a known session whose unwrapped key
  matches the registered key as a carrier re-attach and answers `session_accept` as usual.
  A `session_init` for a known session with a different key is an error and the frame is
  dropped. The relay accepts this because it forgot the session when the old socket
  closed and the same client is the one re-opening it.

### Signaling (inside the existing E2EE session, over the relay)

Three RPC methods and one push, all carried as encrypted `payload`:

| Direction | Shape | Meaning |
|---|---|---|
| client → bridge | `rtc.offer {sdp, ice_servers}` → `{sdp}` | Browser's offer with the ICE servers it fetched from the api; bridge replies with its answer. |
| client → bridge | `rtc.ice {candidate}` → `{}` | Trickled browser candidate. |
| bridge → client | push `{type:"rtc.ice", candidate}` | Trickled bridge candidate. |
| client → bridge | `rtc.close {}` → `{}` | Tear down the peer connection for this session. |

One `RTCPeerConnection` per E2EE session on both sides. **Signaling is pinned to the relay
carrier.** The `rtc.*` methods and the `rtc.ice` push always travel over the relay
carrier, never over a DataChannel, so an ICE restart (credential expiry, network change)
and a fresh `rtc.offer` after a channel loss both work while the DataChannels are down.
The SPA's carrier switch therefore has one routing rule: `rtc.*` goes to the relay
carrier, everything else goes to the active carrier. If the relay carrier is detached
when signaling is needed, signaling waits for the relay re-attach. The DTLS fingerprint
rides in the SDP, which rides in ciphertext, so the sealed session authenticates the DTLS
handshake.
ICE candidates carry IP addresses; they never leave the ciphertext. The relay sees only
that an `e2ee_envelope` passed.

### ICE servers (api)

The Cloudflare TURN key is a long-lived secret. It lives in the `build-app` Secret as
`CF_TURN_KEY_ID` and `CF_TURN_KEY_API_TOKEN` and never reaches a browser or bridge.

New api route, session-cookie authenticated, using the existing auth decorator:

```
POST /api/rtc/ice-servers  →  { "iceServers": [ ...Cloudflare response verbatim... ] }
```

The handler calls
`POST https://rtc.live.cloudflare.com/v1/turn/keys/{CF_TURN_KEY_ID}/credentials/generate-ice-servers`
with `{"ttl": 86400}` and returns the `iceServers` array unchanged. The browser fetches
this once per peer connection and forwards it to the bridge inside `rtc.offer`. The
bridge therefore needs no Cloudflare access and no new api credential; the credentials
are scoped to the user's own device pair and expire in a day. A session that outlives
the TTL does an ICE restart with fresh credentials.

### DataChannels

Two negotiated channels per peer connection, created identically on both sides with
explicit ids so no in-band open handshake is needed:

| Label | id | Ordered | Reliable | Carries |
|---|---|---|---|---|
| `app` | 0 | yes | yes | The app RPC session (`core/session.js`). |
| `term` | 1 | yes | yes | The terminal session (`terminal/session.js`). |

This mirrors today's two-socket split. SCTP streams are independent, so a terminal flood
does not head-of-line block an RPC reply, which is the same property the two sockets
buy today.

**Chunking.** Browsers cap a single DataChannel message (Chrome at 256 KiB; the
SDP-negotiated `a=max-message-size` is authoritative). The relay allows 8 MiB frames and
diffs use them. The DataChannel carrier splits any serialized envelope larger than
`CHUNK_BYTES = 16 * 1024` into ordered parts:

```json
{"part": {"id": 17, "index": 0, "count": 12}, "data": "<slice of the envelope JSON>"}
```

An unchunked envelope is sent as-is; a receiver distinguishes the two by the presence of
`part`. Reassembly is bounded at `MAX_REASSEMBLED_BYTES = 8 MiB` (the relay's frame
cap); an over-limit or gap-having reassembly closes the channel. The channel is ordered,
so parts cannot interleave across ids from one sender.

**Backpressure.** The peer connection holds each channel's writer at
`DC_BUFFERED_HIGH = 1 MiB` of that channel's own buffered bytes: a send past the limit
waits until the peer has taken enough of what the channel already holds, and fails once
the channel is closing. The limit is per channel, so a terminal flood still cannot stall
an RPC reply. The envelope queue behind the writer is unbounded, so a client that will
not drain trades the channel's send buffer for device heap while it stays attached. The
existing bridge-side `term.ack` budget (`TERM_UNACKED_BUDGET_BYTES`, `bridge/src/app.rs`)
applies unchanged on both carriers.

### Bridge peer (`bridge/src/rtc.rs`)

- Crate: `webrtc` 0.20.x (webrtc-rs), tokio runtime, `crypto-ring` provider. No OpenSSL,
  which keeps the bridge's rustls-only stance. `str0m` was considered and rejected for
  now: it ships no TURN client, so the allocation glue would be ours to write.
- The bridge is always the answerer. Per session: build the peer connection from the
  offer's `ice_servers`, create the two negotiated channels, set remote offer, create and
  return the answer, then trickle candidates through the `rtc.ice` push.
- A channel registers no senders when it opens. A migrating browser re-sends
  `session.hello` and re-attaches its terminals over the channel, and admitting those
  frames is what puts the session on the channel carrier: the app's existing `register`
  (`bridge/src/app.rs:394`) replaces the prior sender for that `session_id` exactly as it
  does on a reconnect, which is how push traffic migrates to the new carrier.
- Peer connection close (ICE failure, DTLS close, `rtc.close`, or session end) drops the
  DataChannel senders; the next push fails and the app reaps them exactly as it does for
  a dead relay socket today.

### SPA carrier and migration policy

A `Carrier` interface sits under both `openRelaySession` and `TerminalSocket`:
`{ send(envelope), onEnvelope(fn), onClose(fn), close() }`. The relay socket and each
DataChannel implement it. The two session modules keep their public interfaces
(`{call, deviceId, close}` and the `TerminalSocket` methods), so no caller changes.

Policy, in order:

1. **Connect on the relay first.** Boot, resume, and device switch behave as today. The
   user is live the moment `session_accept` arrives.
2. **Upgrade in the background.** After the app session is live, fetch ICE servers,
   open the peer connection, send `rtc.offer`, trickle. If the fetch or the offer fails,
   log it and stay on the relay; no retry until the next relay reconnect.
3. **Migrate when both channels are open.** The app session re-sends `session.hello`
   over `app`; the terminal session runs `_reattachAll()` over `term`. From then on every
   `call` and every terminal frame uses the DataChannels. Migration is a reconnect from
   the cursor's point of view ("a cursor never crosses connections"), so no new
   dedupe rules are needed.
4. **Keep the relay socket open.** It carries presence (`device_online` /
   `device_offline`), the signaling RPCs, and is the fallback. Its cost while idle is
   the heartbeat.
5. **Fall back on channel loss.** A DataChannel close migrates the session back to the
   relay the same way (hello + re-attach). If the relay socket is also gone, the existing
   `resume()` / `_onLost` paths run unchanged.
6. **Relay loss while the DataChannels are live is not "offline."** The relay reconnects
   in the background and re-attaches the existing session by repeating `session_init`
   with the same session id and key (see "Shared session registry"); the user keeps
   working over the peer path. `App.offline` is true only when no carrier is live. The
   session id and key are minted once per session, not once per socket.

The liveness ping in `terminal/session.js` and `FRAME_PROOF_OF_LIFE_MS` apply per
carrier and need no change beyond running against whichever carrier is active.

### Security and threat model

- **What Cloudflare sees.** TURN allocation source IPs, and DTLS ciphertext when a
  session is relayed. Under DTLS is our secretbox envelope, so even a DTLS break
  exposes only the same metadata the relay sees today.
- **What the relay sees.** Unchanged, and less of it: after migration only signaling
  and presence traffic.
- **New exposure, accepted by decision.** On a direct path each peer learns the other's
  IP. Today the relay hides both. `iceTransportPolicy: "relay"` would restore that at the
  cost of routing every session through TURN; it is not the default.
- **ICE-path MITM.** The DTLS fingerprint is authenticated by the sealed session; an
  attacker on the path cannot substitute a peer.
- **Credentials.** TURN credentials are short-lived, minted only for authenticated users,
  and travel to the bridge inside ciphertext. The TURN key never leaves the api Secret.
- `E2E-MVP.md` §"What the relay sees" and `E2EE Platform Scope.md` get a "second
  infrastructure party" paragraph. Ship criterion 8 ("the relay stored nothing but
  ciphertext") extends to "Cloudflare carried nothing but DTLS ciphertext."

### Deploy and cost

- New Secret keys on `build-app`: `CF_TURN_KEY_ID`, `CF_TURN_KEY_API_TOKEN`. No relay
  manifest changes. No new pods.
- Cloudflare TURN pricing: 1000 GB/month free, then $0.05 per GB of TURN egress to the
  client. Direct paths are free. Usage is visible in Cloudflare's GraphQL analytics
  within 30 seconds; a monthly check goes on the ops checklist.
- Local dev (`deploy/compose.real.yml`) works without Cloudflare: with no TURN key the
  api returns a STUN-only list and direct host candidates carry localhost sessions.

## Stages

Sequential, single worktree, each stage green (tests, clippy, fmt, semgrep, gitleaks)
before the next starts. TDD throughout.

1. **carrier-boundary** — `OutboundEnvelope`, carrier-neutral `SessionSender`,
   `SessionRegistry` owned outside the relay loop. Pure refactor, zero wire change; the
   existing relay integration tests are the gate.
2. **signaling** — `rtc.offer` / `rtc.ice` / `rtc.close` handlers and the `rtc.ice` push
   in `app.rs`, with a stub peer that only records what it was given. This stage also
   gives the registry its multi-carrier semantics: a session ends with its last carrier,
   relay `session_closed` only detaches the relay carrier, and `session_init` for a known
   session re-attaches on a matching key and is dropped on a mismatched one. Tests cover
   unknown-session, double-offer, close-before-answer, re-attach with matching key,
   re-attach with mismatched key, and `session_closed` while a second carrier is live.
3. **ice-servers** — api route, Secret wiring, STUN-only fallback without a key, unit
   tests against a mocked Cloudflare endpoint.
4. **bridge-peer** — `rtc.rs` on webrtc-rs: peer connection, negotiated channels,
   chunker/reassembler, backpressure, DataChannel `SessionSender`. Integration test runs
   a second in-process webrtc-rs peer as the "browser" over host candidates, mints a
   session over the existing test relay, migrates it, and asserts identical RPC results
   on both carriers.
5. **spa-carrier** — `Carrier` interface, DataChannel carrier, upgrade/migrate/fallback
   policy in `connection.js` and `terminal/manager.js`, chunker mirror. vitest with a fake
   `RTCPeerConnection`; browser verification via the `spa:verify` skill against a local
   bridge.
6. **docs-and-ops** — threat-model amendments, deploy Secret, README run instructions,
   the cost check on the ops list.

Model assignment follows the standing policy: Opus 5 for coding and review, Fable only
for stage gates.

## Non-goals

- Cloudflare Realtime SFU, or any always-relayed topology.
- Replacing relay authentication, device pairing, or per-user scoping.
- Multi-device fan-out or more than one bridge per session.
- Forward secrecy, the `crypto_secretstream` upgrade, or any envelope change.
- Per-terminal DataChannels (two channels match today's two sockets; more is a later
  measurement question).
- Mobile or non-browser clients.

## Open questions for review

1. TURN credential TTL: 24 h as specified, or shorter with a scheduled ICE restart?

## Decisions taken during the build

- **The bridge logs the negotiated candidate pair type** (open question 2, closed in
  stage 4): one line per session, the moment its peer connection first carries, naming
  the winning local candidate's type — `host` and `srflx` are direct and free, `relay` is
  TURN egress that is billed. One line per session is what makes "how often is TURN
  actually used" answerable from the logs without a metrics pipeline. It goes to stderr,
  where everything else `rtc.rs` says goes.
