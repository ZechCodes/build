# WebRTC payload encryption removal: audit and proposed contract (#904)

Status: proposed for #906 architecture review, before #909 implementation.
This document changes no running protocol. The implementation must prove the
negative cases below before removing application secretbox calls.

Audit base: `origin/dev` **12b8f869bd94e259666df69f1a8c0cde5c695982**,
fetched 2026-10-29. Branch: `task/904-webrtc-payload-audit`, Build-managed
workspace `bridge-crypto-904`. Only this document is owned by this audit.

## Finding and scope

Removing `decrypt_envelope` from today's channel intake would remove the
proof that authorizes its session-to-carrier association. A clear frame's
`session_id` must never replace that proof. Authenticate the existing
SDP/DTLS peer, explicitly bind each app/terminal opening to its actual
channel, and carry that immutable authority into the shared execution seam.
Then remove application secretbox and ciphertext base64 on those channels.

Keep sealed `session_init`, encrypted `session_accept`, encrypted `rtc.*`
requests/replies/pushes, paired-device key trust, certificate verification,
relay application rejection, ordered app/term channels, chunking and
backpressure. Reuse these primitives; introduce no new cipher or parallel
key agreement. TURN remains an encrypted packet carrier. HTTP/SSE, URL
discovery and the existing durable journal belong to #903/#905/#739 respectively.
Coordinate future state-sync behavior with #887.

Coordinator decision (2026-10-29, #904 task comment): retain existing trusted
relay ownership authorization and carried-session availability. Setup/bind
timeouts are finite; there is no mandatory WebRTC introspection, grant
refresh or 30-second authorization expiry. HTTP has separate finite online
verification because it bypasses the relay. Hosted WebRTC grant/PoP schemes
are outside this delivery. Neither public-key wrapping nor a caller-supplied
account/session identifier is account authorization.

## Current trace at the audited base

| Boundary | Evidence and implication |
| --- | --- |
| Device trust | `skriftapp/buildapp/devices_controller.py::register` verifies the Ed25519 registration challenge covering identity key, transport key and pairing code hash; approval binds the device to an account. `spa/src/devices.js::pinnedDeviceTransportKey` reads the authenticated API device list/cache. `connection.js` supplies it to the rendezvous. This is API-pinned trust, not an independently pinned browser identity key or relay-supplied key. Preserve the pairing fingerprint workflow. |
| Browser account authorization | `bridge/src/bin/relay.rs::authenticate_client` validates the gateway token through the configured hosted API. `relay_server.rs::RelayState::open_session` checks its user against the target device owner and prevents a live client's session-ID collision. Subsequent routing is checked against that client/device session map. This trusted relay/API path is today's authorization source. |
| Session key proof | `transport/js/src/index.js::createSessionInit` seals a random 32-byte session key to the device X25519 transport key. Anyone knowing that public key can construct one. `bridge/src/transport.rs::open_session_init` unwraps the key; it does not identify an account. `carrier.rs::FrameIntake::open` registers it and returns a secretbox `session_accept`. `rendezvous.js::mint` verifies the accept, but should also compare the opened result to the exact pending session ID. |
| Authenticated signaling | `sessionSwitch.js` routes `rtc.*` to the rendezvous. `sessionRpc.js` encrypts with the session key. `FrameIntake::accept` decrypts before `app/rtc.rs` dispatches off the app lock. `SessionPeers` registers `(Opening, peer)` and rejects old-opening signaling against a replacement. Offer SDP and answer SDP therefore travel inside the existing encrypted session. |
| Actual DTLS peer | `peerLink.js::offer` applies the authenticated answer with `setRemoteDescription`; `rtc.rs::WebrtcPeer::answer` applies the authenticated offer. The vendored `rtc/src/peer_connection/transport/dtls/mod.rs:133` compares the actual presented certificate's SHA-256 digest against SDP fingerprints. Its `insecure_skip_verify(true)` skips public-CA verification; the separate fingerprint callback remains required. The bridge does not disable that callback. Preserve and test it. |
| Carrier association | `carrier.rs::SessionRegistry::admit` looks up the outer session ID globally, decrypts with its key, then lazily inserts the carrier. `DataChannelCarrier::ride` creates an unbound channel handle; its session ID is diagnostic context, not enforced inbound identity. `RidingChannel::accept` reassembles JSON and passes an envelope to that global lookup. A channel can presently carry a different session when that session's key proof is valid. |
| Outgoing application frames | `SessionSender::seal` stamps `bridge_epoch`, builds an encrypted inner Frame and wraps ciphertext/nonce. Replies, receipts, ping, ChangeBus, terminal pushes and authenticated signaling share it. Removing this function globally would expose signaling and lose epoch stamping. Split representation at the adapter boundary instead. |
| Terminal multiplexing | `connection.js::mintTerminalSession` mints a separate ID/key on the device rendezvous. `terminal/session.js` gives its own `createSessionRpc` the same peer's `term` carrier. It does not negotiate a second DTLS peer. `term_id` multiplexes PTYs inside that terminal session. App-ID-only binding would break or weaken this path. |
| Replay and lifetime today | `transport.rs::decrypt_envelope` checks version, session match and frame enums; it does not remember `message_id` or enforce `created_at`. ARCHITECTURE.md explicitly records the lack of replay protection. `Opening`, carrier release gates, SQL scopes and SPA `bridgeEpoch.js` provide other lifetime fences. Preserve them; do not claim existing secretbox prevents replay. |
| Relay refusal | `FrameIntake::accept` refuses non-`rtc.*` relay data before dispatch; encrypted session-close is an existing control exception. The relay cannot classify inner methods. Preserve enforcement in bridge intake, including with a malicious relay forwarding an application-shaped object. |

## Account provenance and the shared principal

`execution::ClientSessions` (#903, proposal head
`60fcb19d8f359c0d5f3dc728d4afb44305ece7c0`) owns client opening/binding fences; this
name avoids collision with the agent-runtime session registry. Execution
accepts opaque verifier-produced owner context, never a deserializable
`VerifiedCaller` or a struct minted by application request fields.

For the existing single-owner bridge, both verified owner transports map to
`app::mutations::DEVICE_USER` (`"device_user"`). Account ID, credential family,
access-token ID, session ID, peer ID, `client_id` hint and bearer bytes are
not durable operation owners. `writes.status`, `writes.subscribe`, same-ID
retry, and conflict checks use the existing journal and owner/scope checks.
#739 owns existing durable replay/idempotency; #887 owns future state sync; this change must not add a second
journal, receipt stream, event cursor or replay cache for mutations.

### Trusted relay provenance (selected baseline)

Mint a `RelayValidatedOwner` context only at the configured,
TLS-authenticated relay device connection's validated `session_init` receive
path, after its existing API-backed ownership routing. A raw `SessionInit`,
an arbitrary channel-borne init, an envelope and a test-only channel factory
cannot call that constructor. The trusted statement is "this device's owner
passed relay authorization", not an authenticated account UUID invented by
the bridge. Do not expose a fake account ID just to fill a common struct.
The shared owner abstraction must represent that provenance honestly.

Capture relay connection identity and the Opening allocation; require key
proof and the authenticated offer on that opening. Once bound, preserve
today's carried-session lifetime after the rendezvous closes. This retains
the existing trust in the configured relay/API for account authorization and
does not claim protection against a malicious authorized relay creating its
own owner session. Changing that trust model is outside this migration. Finite bootstrap/bind
deadlines do not impose an expiry on an established carried session.

## Finite peer and channel handshake

This is the new clear-mode path. The legacy compatibility path below retains
its current encrypted intake and signaling; it cannot acquire a clear lease.

Proposed transport discriminator: `dtls-json-v1`. There are no clear
application frames before `Bound`. Local tokens below are allocated by the
bridge; names in JSON cannot construct the internal token.

1. **Mint / Keyed:** preserve the existing 8 s rendezvous-open and 10 s
   accept deadlines, sealed SessionInit and secretbox accept. Check
   outer/inner/pending session IDs and target device agree before registration;
   the SPA compares `openSessionAccept().sessionId` to its exact pending mint.
   Mint verifier-produced relay-owner provenance only through the trusted
   relay adapter above. Same-key rendezvous reattach preserves the Opening;
   a new opening is a new allocation even if its textual session ID is reused.
   Keyed is not authorized for application execution. No new SessionInit
   fields, cipher format or SessionAccept metadata are needed.
2. **Offer / Authorized:** within the existing 15 s peer-setup deadline,
   encrypted `rtc.offer` advertises `payload_protocols` in preference order
   (`dtls-json-v1`, `secretbox-v1`), a fresh client `peer_nonce`, SDP
   and ICE servers. Require the authenticated frame's current Opening and
   verifier-produced relay-owner provenance. Capture the exact original SDP
   hash before the existing ICE-policy/mDNS transforms; validate these never
   change the DTLS fingerprints/application channel section. Do not hold
   the app mutex. Recheck Opening/peer generation after each await. Repeated
   exact offer is idempotent, not permission to create another peer or change
   its identity. A failed attempt retires its bounded provisional state.
3. **Peer identity:** the authorized peer allocation captures Opening,
   protocol, initial-offer hash, peer nonce, both normalized SHA-256 DTLS
   fingerprints and negotiated channel roles. Its encrypted answer returns
   exact answer SDP, its hash, a random 32-byte `peer_token`, peer nonce and
   selected protocol. The bridge allocation retains the actual Opening token;
   a wire session ID is only an equality check. Client verifies those against
   its outstanding offer before applying the answer. Hashes use SHA-256 of
   exact UTF-8 SDP bytes, encoded as lowercase hex; nonces/tokens use canonical
   base64. Use a fixed domain-separated JSON shape with the existing transport
   canonical encoding for proof payloads. `setRemoteDescription` plus the existing certificate
   fingerprint verification authenticates the actual DTLS endpoint. Missing,
   mismatched or unsupported fingerprints fail closed. DataChannel `OnOpen`
   alone never constructs a verified owner context.
4. **Bind / Bound:** app bind must finish within 10 s after channel open.
   A terminal bind attempt has a 10 s deadline from its explicit `bind_begin`
   control (session ID + parent peer token); an unused term channel need not
   bind at all and never closes a healthy app peer merely for being idle.
   On a real channel with an active bind attempt the bridge
   emits a fresh 32-byte bind challenge as a small control message. Before
   any ordinary frame, client returns a secretbox control proof using that
   channel's intended session key. Domain-separated payload contains
   `channel_bind_v1`, session ID, peer nonce/token, initial-offer and
   answer hashes, both fingerprints, actual negotiated channel ID and label,
   challenge and protocol. The bridge compares it against the reader's
   captured peer allocation/channel, consumes the challenge once and installs
   a `VerifiedPeerBinding` passed to `execution::ClientSessions::bind_peer`,
   which returns the unforgeable `PeerLease` required for clear intake. This is a use of the
   existing encryption primitive for one control proof, not encryption of
   application traffic. The authenticated bind acknowledgment is under the
   same key and echoes the exact tuple. Client waits for that acknowledgment.
5. **App role:** the app channel (id 0) binds only the offer's app Opening.
   Only after bind acknowledgment may `session.hello` and ordinary RPCs run.
   The old API greeting/epoch check still runs before product dispatch.
6. **Terminal role:** terminal session is separately minted via the same
   device's rendezvous. On `term` (id 1), prove its own key and Opening
   identity against the existing authorized parent `peer_token` and a fresh
   term bind challenge. Its authority comes from its own verified relay Opening
   and K_term proof, plus the exact authorized parent-peer association. Parent
   app authority alone never authorizes a terminal Opening. Require matching
   paired-device owner contexts; SessionInit alone supplies none. No second DTLS
   negotiation, no reuse of the app key and no binding to another device/peer. One terminal session binds
   this channel at a time; replacing it atomically retires the previous term
   binding and rejects its pending frames. A failed replacement proof never
   binds the new session; preserve a still-valid old binding until successful
   atomic replacement. A failed initial term bind retires only that attempt,
   never a healthy app binding. `term_id` remains PTY demux only.
7. **Operation:** keep application input unwrapped by secretbox but strictly
   parsed after this reader's captured PeerLease is checked. Outgoing frames use
   its captured binding; never route clear output by global session ID.
   Preserve receipts, priorities, SQL credits, observation scheduling,
   terminal backpressure, API error envelopes and bridge-epoch stamping.

New channels may open before the authenticated answer is fully installed.
They may receive only a challenge/proof while provisional; installation of
Bound must wait for both the authenticated transcript and verified DTLS
association. App setup timeout closes its provisional peer/channels; a term
setup timeout retires its attempt. Keep one pending challenge per role, no
unbounded frame buffer while unbound; cap control proofs at 64 KiB. The
bridge must derive verification from native peer facts and the authenticated
transcript, never an incoming clear frame or a claimed fingerprint.

### Replay, replacement and downgrade rules

The clear JSON wire frame is `{protocol:"dtls-json-v1", binding_id, seq,
frame_type:"data"|"close", payload}`. `binding_id` is a random bridge-issued
bind acknowledgment token, scoped to that exact Opening/peer/channel. The
reader already holds the real identity; this field is an equality check,
never a lookup that grants another identity. Per direction/per bound channel,
`seq` is an integer starting at 1 and increasing exactly once per complete
frame; reject duplicates/gaps/wrong binding before receipt or scheduling.
Use the JS-safe integer range and close before exhaustion. Assign the number
inside the serialized writer, after cancellation checks, to avoid gaps from
cancelled requests. Close a channel on partial-send/reassembly failure.

DTLS record replay protection (configured in vendored DTLS), authenticated
certificate possession and ordered reliable SCTP remain the network replay
boundary. Sequence checks and fresh binding tokens additionally prevent old
logical frames being admitted twice or moved between adapters/openings. An
authorized malicious owner can issue a fresh operation with new sequence;
mutation deduplication still belongs to the durable journal.

Encrypted signaling keeps its present cipher and envelope version. Add
Opening/peer token to clear-mode `rtc.ice`, `rtc.close` and
restart offers; authenticate and compare before acting. Only ICE-credential
renegotiation on the same native peer with unchanged DTLS fingerprints and
protocol is allowed under the existing binding. Serialize offer processing;
add a monotonic negotiation counter to clear-mode controls and reject stale
controls. Keep the accepted negotiation high-water mark on the Opening,
including across native-peer teardown; replaying an older offer cannot create
a replacement. Retain current ICE generation/ufrag candidate checks. Never let an old close control tear down a new peer.
A new native peer/certificate requires fresh peer/channel bindings to a still-live
Opening. It does not require changing the existing session key, minting a new
hosted account grant or extending a retired Opening. Preserve recovery
behavior while rejecting every old peer token.

**Pre-answer ICE sequencing (#906 review qualification):** arm a bounded
candidate queue before `setLocalDescription`, since gathering/trickle can
start before `rtc.offer` and before the client knows the selected peer token.
For a pending clear-capable negotiation, hold outbound `rtc.ice` until its
authenticated answer has selected the codec and authenticated the matching
peer nonce/token and negotiation generation. Then flush in gathering order,
including the end-of-candidates marker, tagging clear-mode candidates with
that selected token/generation. Recheck the current Opening/peer/generation
before each send; a replaced or cancelled round never flushes into its
successor. Ordinary ICE restarts use the same gate for the pending generation
even when the existing peer token is known.

The bridge also holds its new-generation candidate pushes until its
authenticated answer has been enqueued, then emits them with the matching
token/generation. If an authenticated candidate push arrives at the SPA
before that answer is processed, buffer it against the pending negotiation;
apply it only after the answer's token/generation match and remote SDP is
installed. Never apply an early candidate to the previous generation.

Each pending direction is capped at **128 candidate objects and 64 KiB of
serialized UTF-8 candidate data**, including candidates retained by the
existing relay-hold policy; retain at most one end marker. Queue lifetime
ends at the existing 15 s setup deadline, without extending it for answer
arrival or flushing. Overflow, timeout, cancellation, refused answer or peer
replacement clears that round's queue and fails its setup; do not silently
drop early candidates or create an unbounded retry queue. Once legacy is
selected, use the existing candidate shape and relay-hold behavior, without
new token requirements. Existing legacy-only peers retain their current
trickle behavior. TDD coverage must deliver candidates synchronously during
`setLocalDescription`, delay the answer, exercise an early bridge push, then
prove ordered flush under the selected generation, stale-round suppression,
both queue caps and unchanged legacy routing.

On replacement, atomically mark old binding noncurrent and cancel its
challenge/read admission before installing a new one. Queue entries, replies,
ChangeBus and terminal senders retain Opening + peer allocation + carrier
allocation, not only strings. Every late callback, release and teardown uses
allocation equality: closing old peer A cannot release new peer B's carriers
or end B's opening. Rebinding a textual session ID must not revive A. For a
stable ICE restart with the same DTLS association, keep the tokens/sequence;
if a native implementation replaces the association, perform fresh binding.
SPA pending calls retain source-carrier, SQL and bridge-epoch fences.

Keep an explicitly negotiated legacy encrypted codec for supported old/new
combinations; do not make bootstrap update-required the default. Protocol
selection occurs inside the authenticated offer/answer exchange, before
channel commitment, and is pinned to the exact peer allocation:

| SPA / bridge | Selected codec |
| --- | --- |
| New / new | Highest mutually offered protocol: `dtls-json-v1`; authenticated answer echoes selection and peer nonce/token. |
| Old / new | Missing protocol advertisement means legacy-only `secretbox-v1`; keep the existing encrypted intake and session-key proofs. |
| New / old | The new offer explicitly permits `secretbox-v1`. An old answer without selection is interpreted as legacy-only before commitment. No clear frame has been sent or accepted. |
| Old / old | Existing encrypted protocol unchanged. |

Do not auto-detect the codec from incoming JSON. Absence of selection is a
legacy compatibility signal only on that original pending offer, never an
instruction to downgrade a peer after clear commitment. An altered or
unsupported explicit selection, wrong echoed nonce, failed clear bind, or
clear timeout fails that attempt; do not retry legacy on the same attempt.
ICE restarts preserve the selected codec. A replacement native peer on a
still-live Opening may retain its session key but needs fresh peer allocation,
nonce/token, authenticated selection and channel binds. Retire old leases
atomically; no old token or sequence survives into the new allocation.

A new bridge checks relay provenance for both modes. Legacy mode retains its
existing encrypted wire shape and its existing replay limits; do not pretend
that compatibility traffic has the new clear-mode sequence guarantees. The
new handshake does not change API payload schemas. Advertise a bootstrap
transport capability in the authenticated answer; a `session.hello` capability
may also describe it but is too late to select it. #906 may ratify an additive
API capability/version fixture update with #903; no gratuitous major API
break or transport encryption-library version bump is necessary.

## Relay and framing invariants

Only encrypted `rtc.*`, init/accept and existing relay controls may cross the
relay. Raw/clear `data`, handshake proofs, session hello, ping, terminal and
application close from a channel cannot be smuggled into its parser. Keep
the existing unavailable/nonretryable `relay_is_not_a_data_plane` refusal
for valid encrypted application requests. Reject clear objects before
looking for a session or constructing execution authority. The relay's
64 KiB limit and channel limits remain independent.

Keep text JSON as the first clear-mode codec. Current binary WebRTC messages
are decoded as UTF-8 JSON in `RidingChannel::accept`/`carrier.js`; there is no
native binary application codec to preserve. Keep terminal byte fields and
file/attachment content base64 where their RPC schemas require it. Removing
ciphertext base64 does not authorize changing those schemas. A binary codec
is a later, separately versioned optimization.

Keep `rtc/chunk.rs` / `core/chunk.js`'s 16 KiB UTF-8 splitting, ordered
`{part:{id,index,count},data}` wrapper, code-point-safe slicing and 8 MiB
reassembly cap. Keep one reassembler/serialized writer per channel and the
1 MiB buffered-amount threshold, with app-only stall watch. Ensure the new
whole-frame JSON never begins with the reserved `{"part":` prefix, and
validate total frame size for unchunked inputs too. Sequence/binding checks
run after full reassembly and before receipt/dispatch. Close on malformed,
interleaved, oversized or truncated chunks. Reset buffers with the exact
retired binding; no partial message crosses into its replacement.

## Proposed touched-file ownership (implementation only after #906)

| Owner | Exact existing files / proposed new files | Boundary |
| --- | --- | --- |
| #904 audit | `planning/v2/WebRTC Payload Encryption Audit.md` | This commit only. |
| #903 execution slice | `bridge/src/execution.rs`, `bridge/src/execution/client_sessions.rs`, `bridge/src/execution/dispatch.rs` (proposed); `bridge/src/lib.rs`, `bridge/src/carrier.rs`, `bridge/src/carrier/dispatch.rs`, `bridge/src/carrier/sql.rs` | Own shared scheduler, verified owner abstraction, ClientSessions, Opening/binding fences, plaintext semantic frame and neutral ReplySink. #909 consumes these APIs; it does not duplicate them. Coordinate exact extraction paths with #903 before editing. |
| #905 auth slice | Auth/verifier, hosted grant/controller/store/migration and `spa/src/api.js` changes selected by #905 | Own HTTP online authorization and expiry only. Its verifier and the trusted relay adapter produce distinct internal proofs of the same paired-owner authority. No HTTP token is accepted as a WebRTC binding token. |
| #909 bridge RTC slice | `bridge/src/rtc.rs`, `bridge/src/app/rtc.rs`, `bridge/src/relay.rs`, `bridge/src/carrier.rs`; proposed `bridge/src/rtc/binding.rs`, `bridge/src/rtc/binding_tests.rs` | Own provenance adapter, authenticated offer/answer fields, real peer/channel token capture, finite binds, clear codec, RTC outbound path. Shared `carrier.rs` edits follow #903 extraction, never parallel ownership. |
| #909 SPA RTC slice | `spa/src/core/rendezvous.js`, `spa/src/core/peerLink.js`, `spa/src/core/carrier.js`, `spa/src/core/sessionRpc.js`, `spa/src/core/session.js`, `spa/src/core/sessionSwitch.js`, `spa/src/connection.js`, `spa/src/terminal/session.js`, `spa/src/terminal/manager.js`; proposed `spa/src/core/peerBinding.js` | Own accept verification, narrow signaling/clear codec choice, bound readiness, parent/term lifetime propagation and terminal replacement. |
| #909 protocol/tests/docs | `bridge/src/carrier/epoch_tests.rs`, `bridge/src/rtc/backpressure_tests.rs`, `bridge/src/rtc/superseded_tests.rs`; `spa/test/carrier.test.js`, `spa/test/chunk.test.js`, `spa/test/rendezvous.test.js`, `spa/test/sessionRpc.test.js`, `spa/test/sessionSwitch.test.js`, `spa/test/peerLink.test.js`; proposed `spa/test/peerBinding.test.js`, `fixtures/api/rtc-binding-v1.json`; `ARCHITECTURE.md`, `planning/v2/Strict P2P Transport Spec.md`, `planning/v2/WebRTC Transport Spec.md`, `planning/v2/WebRTC Primitives.md` | Own migration fixtures, negative tests and amendments. Coordinate shared adapter/terminal test files with #903/#905. Leave transport vectors/Python crypto protocol and encryption primitives intact; the SPA checks the existing accept result. |

Bound bind-proof payloads use distinct fixed domain tags for proof and
acknowledgment (`channel_bind_v1` / `channel_bound_v1`) to prevent reflection.
No vendor modifications are needed for certificate verification. Do not edit
`app/mutations.rs`, mutation persistence or sync-cache code to implement a
transport credential as a durable owner. A necessary API visibility change
belongs to the shared execution slice and must keep journal behavior.

## Required implementation evidence (TDD)

Write failing negative integration tests against the actual intake/adapter,
then implement and show them pass. This design-only change adds no fake test
claiming that a future protocol is implemented.

| Test group | Required rejection / preservation witnesses |
| --- | --- |
| Peer identity | Tampered authenticated SDP/answer, certificate not matching SDP, missing fingerprint, wrong peer token/transcript; real native peer mismatch test plus a successful local negotiated-channel exchange. Existing vendored fingerprint callback must remain enabled. |
| Principal provenance | Bare public-key SessionInit has no execution authority. Untrusted channel init and caller-provided VerifiedCaller/account ID cannot create owner context. Correct configured relay path can create only its scoped relay-owner context; HTTP evidence cannot be fabricated or transplanted into a relay/RTC proof. |
| Binds | Frame before acknowledgment; wrong key, Opening, challenge, channel ID/role, session ID, device, offer/answer hash; app key on term and term key on app. Correct separately minted term session on the authenticated parent works. Failed proof adds no carrier ride and produces no receipt or side effect. |
| Replay/downgrade | Replayed bind challenge, duplicate/out-of-order sequence, prior binding frame, altered protocol/mode, old offer/close/candidate on replacement. New SPA/old bridge and old SPA/new bridge use negotiated legacy before commitment; new/new uses clear. Failed clear commitment never retries legacy or relay. |
| Lifetime | In-flight bind completes after close/replacement/timeout; old peer teardown overlaps new peer install; same session text reopens under another key; channel closes between validation and admission/output. Only the current Opening/peer/carrier executes or emits. ICE-only restart retains its valid binding; changed certificate/native association cannot. |
| Authorization/lifetime compatibility | Rendezvous/hosted API outage after successful bind does not expire a working peer. Key possession without trusted relay provenance cannot bind. HTTP grant expiry closes only its own bindings. Neither transport can close another owner opening merely by naming its session or credentials. |
| Relay | Clear data/hello/bind/terminal objects never execute on relay; encrypted app request gets existing refusal; encrypted rtc offer/ice/close and accept still work. Wrong-key data does not bind a carrier. |
| Framing/performance behavior | Unicode and large JSON at 16 KiB/8 MiB boundaries, chunk interleaving/truncation, binary UTF-8 decoding, unchunked size cap, cancellation before sequence assignment, partial send failure, simultaneous app RPC and terminal flood. Keep SQL credits and scoped subscriptions, bounded push queues and epoch checks. |
| Durable equivalence (#739/#903; #887 sync coordination) | Same operation ID first admitted over WebRTC then retried/status-read over HTTP has one DEVICE_USER journal result; changed credentials/session do not fork it; changed semantic request still conflicts. No terminal-input retry. |

Run normal bridge/SPA gates under `nice -n 10`, plus existing transport
compatibility tests to prove signaling crypto remains unchanged. Include semgrep before ready and
gitleaks before any authorized branch push; no dev/main push, live bridge
rollout or shared-source merge is authorized by this audit.

## Baseline and performance measurement plan

Checks on the audited base (2026-10-29), all with exit code **0**:

| Command (under `nice -n 10`) | Result |
| --- | --- |
| `cargo test --locked --lib carrier::` from bridge | 130 passed; includes relay rejection, no-ride-on-wrong-key, opening/teardown, ping, scheduling, SQL and epoch coverage. |
| `cargo test --locked --lib rtc::` from bridge | 212 passed; includes app RTC handlers, chunking, native peer and superseded-peer tests. |
| `cargo test --locked --lib transport::` from bridge | 16 passed. |
| `cargo test --locked --lib relay_server::` from bridge | 36 passed, including gateway session ownership/collision and device authentication checks. |
| `npm test --prefix transport/js` | 8 passed, including JS/Python interop and transport vectors. |
| `node node_modules/vitest/vitest.mjs run test/carrier.test.js test/chunk.test.js test/rendezvous.test.js test/sessionSwitch.test.js test/sessionRpc.test.js test/peerLink.test.js` from SPA | 6 files, 236 passed. |
| `semgrep --config auto --error "planning/v2/WebRTC Payload Encryption Audit.md"` | 47 rules, 1 target, 0 findings; documentation-only scope. |
| `git diff --check` | Clean. |

Installed existing lockfiles with Node 22.23.4 and npm 12.0.2 under nice; no
manifests or lockfiles changed. These checks exercise the existing protocol,
not the proposed new handshake. Full product
gates are not a meaningful implementation claim for a documentation-only
audit. A missing new-mode negative test is a blocker for #909, not a passing
security-checklist entry here.

A local existing-code JS codec baseline used Node 22.23.4 on an AMD Ryzen
Threadripper 2950X, Linux 7.2.5-3-omarchy. For each deterministic repeated-`x`
text payload, run 10 warm-ups then 40 samples of encryptFrame -> JSON.stringify
-> JSON.parse -> decryptEnvelope; validate text length each time. Timing
includes JSON/crypto, excludes splitting/network. Sizes include actual
`splitEnvelope` output. Nonces/UUIDs use ordinary random production generation.
The script and full output accompany the task evidence; exit code 0.

| Text bytes | Envelope bytes | Chunk-wire bytes / count | Roundtrip p50 / p95 / max, microseconds |
| ---: | ---: | ---: | ---: |
| 0 | 460 | 460 / 1 | 269.6 / 371.8 / 775.4 |
| 1,024 | 1,826 | 1,826 / 1 | 238.8 / 321.4 / 1,540.7 |
| 16,384 | 22,306 | 22,418 / 2 | 864.3 / 1,208.7 / 1,358.3 |
| 1,048,576 | 1,398,562 | 1,402,784 / 86 | 49,985.6 / 66,711.9 / 94,361.2 |

This small sample is an implementation comparison seed, not a stable CPU
budget or network-latency measurement; with 40 samples the nearest-rank p99
is the maximum. No clear-mode, native-peer throughput or mobile claim is
made before implementation.

Capture before/after measurements on the same base/head, release build,
Node/browser, CPU, fixture payloads and connection path. Separate a local
no-network codec benchmark from a native peer loopback exchange. Measure
warm 0/1 KiB/16 KiB/1 MiB payloads and terminal chunks under simultaneous RPC
load: serialized UTF-8 bytes, full chunk-wire bytes, encode/decode CPU and
allocations, p50/p95/p99/max RPC latency, terminal throughput, peak queued
bytes and scheduler wait. Add cold connection setup and bind overhead; an established WebRTC
session requires no new hosted verification traffic. Report sample count,
warm-up, seed and exit status; do not claim a universal latency gain from
removing a cipher. For plaintext length N, old secretbox contributes a
16-byte MAC plus base64 expansion, a 24-byte nonce encoded as base64 and
envelope JSON; the clear protocol's binding/sequence also has overhead.
Measure actual messages rather than claiming all base64 disappears.

Hardware LAN, TURN, flaky reconnect and mobile-browser measurements require
separate approved fixtures; no live bridge is used to obtain this baseline.
