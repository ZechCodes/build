# Dual transport architecture review — #906

Review base: `12b8f869bd94e259666df69f1a8c0cde5c695982`, fetched from
`origin/dev` on 2026-10-29. Review-only branch:
`task/906-architecture-review`. This document records the architecture contract
for #902, not implementation acceptance or deployment authorization.

## Reviewed revisions and verdict

Verdict: **approve the architecture for implementation under this consolidated
contract**, including the ICE sequencing qualification below. This is not
approval of unimplemented security behavior or release. The coordinator owns
the final freeze, reviewed-base handoffs and dispatch; #906 stays In review.

All three proposals were read at these exact heads, against the base above;
saved Build snapshot contents and changed-file lists were checked:

| Task | Exact head | Reviewed files / saved snapshot |
| --- | --- | --- |
| #903 | `60fcb19d8f359c0d5f3dc728d4afb44305ece7c0` | `docs/shared-execution-contracts.md`; `c13dee7d-00f5-4a52-8986-c7bfe395368e` |
| #904 | `4fd40ad17a302cb606ed014d168873513056e7c0` | `planning/v2/WebRTC Payload Encryption Audit.md`; `fe5bcdef-b13d-4d83-b10b-7954aed1023b` |
| #905 | `5ec99d0ce83d7af5b3995bdc79f310006035507f` | `planning/v2/bridge-http-905/{PROPOSAL.md,client-coverage.md,openapi.yaml,security.feature}`; `0e345cc2-ef4a-4a65-8a1b-2932e4bf3994` |

These are documentation-only proposals. The corrections to earlier drafts
resolve the architecture blockers: owner provenance, stable operation principal,
terminal isolation, compatibility, direct-URL trust and honest recovery.
The lifecycle OpenAPI skeleton and acceptance scenarios specify future work;
they do not demonstrate completed resource coverage or passing runtime tests.

This addendum takes precedence over conflicting proposal wording and ownership
placeholders. #904's `91024b1a2d381266cf32df187e12906f51674f92` was reviewed in
full, then its 33-line ICE clarification delta to the final head was reviewed.
There are no remaining must-fix architecture blockers under this addendum.

The supplementary acceptance matrix is #907 at
`35b2395104ed01cdebf5deee9dd5d63c39d1e955`,
`planning/v2/Bridge Dual Transport Acceptance Matrix.md`. Apply the WebRTC
authorization and transient-SSE qualifications below to that matrix.

## Scope and existing evidence

The requested change removes the application payload cipher from authenticated
WebRTC DataChannels and adds an explicitly enabled account-authenticated HTTP
adapter alongside WebRTC. It does not remove DTLS/TLS, encrypted signaling,
device pairing or encrypted push notifications.

The base establishes the important seams:

| Evidence | Consequence |
| --- | --- |
| `carrier.rs::SessionRegistry::admit` decrypts before adding a carrier binding | A clear frame cannot establish its own session association by naming an ID. |
| `rtc.rs::RidingChannel` owns a carrier; `SessionPeers` already tracks `Opening` identity | Bind the actual peer allocation and channel to that opening before decoding clear application traffic. |
| `vendor/rtc/src/peer_connection/transport/dtls/mod.rs` checks certificate SHA-256 against SDP | Keep the existing certificate verification and encrypted SDP path; no replacement cryptographic handshake is needed. |
| `relay_server.rs::RelayState::open_session` checks client/device ownership; `relay.rs::open_session` receives the validated relay message | Owner provenance comes from trusted configured relay intake, not public-key encryption of SessionInit. |
| `app/mutations.rs::DEVICE_USER` is `device_user` | Preserve that stable single-owner journal identity across transports. Session IDs and bearer-token IDs are not durable principals. |
| `api/writes.rs` advertises an empty ack-confirm method allowlist at this base | Do not promise universal durable or exactly-once HTTP mutations. |
| `changes.rs` coalesces transient updates; #887 owns future persistent scoped sync | Do not invent a second SSE journal or expose ChangeBus revision as a resume cursor. |
| `desktop/src/security-policy.mjs` strips Authorization outside hosted API routes; hosted CSP limits connect-src | Direct browser and desktop HTTP require explicit policy integration as well as CORS. |

## Shared execution boundary

Transport adapters authenticate, frame and deliver. Shared execution authorizes,
validates typed method parameters, admits work, dispatches to existing services
and produces outcomes. Domain handlers do not acquire HTTP, encryption or
carrier-mode branches.

Keep the existing method registry and typed domain handlers; do not introduce
a second method catalogue with different validation or business logic. A
validated request wrapper may contain normalized JSON at the codec boundary;
that is not permission to replace typed handlers with unvalidated catch-all
values or rewrite every domain service.

Separate these lifetimes:

- Verified caller authority, constructed only by the respective trusted
  adapter/verifier. The current owner maps to the same `device_user` principal.
- An execution opening and exact request/reply binding. An old carrier or HTTP
  response cannot publish into its replacement.
- Subscription/output bindings, including independent app and terminal output.
  An HTTP response is not an SSE subscription and a keepalive is not authority.
- Durable operation identity where a particular method actually enables it.
  Disconnect and timeout do not erase a committed/uncertain operation.

Authenticate and authorize before all routes, including immutable snapshot
fast paths. Retain the shared SQL and observation executors, bounded per-opening
queues, queued-only folding, terminal ordering, liveness/signaling fast paths
and update admission leases. HTTP must not create a fresh unlimited execution
pool or obtain fairness capacity by opening one request after another. Adapter
body/connection limits supplement the shared scheduler.

Transport loss cancels queued work and delivery bindings according to existing
semantics. Work already producing external effects must finish or reconcile;
an HTTP cancellation is not proof of rollback. A request deadline after send
means uncertain delivery for a legacy write, not automatic retry permission.
Only the advertised per-method durable contract permits an operation-ID retry.

## WebRTC minimum migration

Keep sealed SessionInit/session_accept, secretbox `rtc.*` signaling, the
API-pinned bridge transport key, and the existing WebRTC DTLS stack. The
configured authenticated relay's owner-validated intake creates owner context;
generic SessionInit parsing cannot do so. A relay remains trusted for account
authorization as it is today. This migration does not claim protection from a
compromised authorization service or trusted relay minting an authorized-looking
session.

The invariant for clear data is an unforgeable internal binding to the actual
RTC peer allocation, opening generation, negotiated channel role and selected
codec. The SDP fingerprint arrives authenticated through encrypted signaling;
the DTLS stack verifies the corresponding certificate before DataChannel data
is admitted. Neither a clear frame's session ID, a UUID client hint, an ICE
address, nor a mere connected callback is sufficient evidence by itself.

The terminal's separately minted session must prove possession and bind to that
same peer and its own opening before clear terminal requests/output. App and
term role bindings are not interchangeable. A finite setup/bind deadline bounds
unbound state. Closing or replacing a peer revokes only its exact allocation;
old callbacks, replies and channel messages cannot end or reach a newer one.
ICE restart preserves a live peer's identity only under the specified
authenticated negotiation rules.

Negotiate the codec explicitly inside authenticated signaling. Selection is
immutable for the bound peer; do not infer it by parsing payloads, retry a failed
clear handshake as legacy, or send clear application traffic over the relay.
Retain explicit legacy encrypted support for supported old/new client/bridge
combinations; select it only before commitment to the new mode. A first
connection cannot depend on an application `session.hello` that itself needs
the new channel codec; bootstrap negotiation must be self-contained.

Peer replacement may retain a live Opening and session key, as current
`rtc.close`/recreate does, while allocating a fresh peer nonce/token and fresh
channel binds. Retire old bindings atomically. A replacement certificate must
be authenticated through the new offer/answer, not silently accepted under the
old peer token. An unused terminal role remains unbound; failed terminal setup
does not close a healthy app binding.

Ratify #904's one-time role-specific secretbox proof and acknowledgment, with
distinct `channel_bind_v1` / `channel_bound_v1` domains, fresh 32-byte challenges,
exact transcript hashes and actual channel/peer comparison. Reuse existing
session keys and NaCl/secretbox implementations; retain native DTLS fingerprint
verification. No new key agreement or per-message application cipher is needed.
Fixed parsed proof fields are compared to captured facts. Existing
`transport/js/src/index.js::canonicalJsonBytes` sorts keys recursively and
`bridge/src/transport.rs::canonical_json` uses serde_json's sorted map. Their
existence is verified, but proof validation does not require cross-language
byte equality after reserialization: decrypt, parse, validate types and compare
the expected fields. Reuse these helpers without inventing a new canonical-JSON
or signature protocol. Exact original UTF-8 SDP hashing is a separate check.

Its per-channel safe-integer sequence and binding token are small local fences;
the per-Opening negotiation high-water mark fences delayed encrypted controls
across replacement. These are proportionate to moving intake away from the
global encrypted-session lookup. They are not durable replay machinery, account
freshness or a claim of exactly-once effects. DTLS remains the network security
boundary; the legacy codec retains its existing replay limits. Keep this state
bounded to live openings/channels, not a new persistence subsystem.

**Required sequencing qualification for #909:** current
`spa/src/core/peerLink.js` can emit trickle ICE after `setLocalDescription` and
before `rtc.offer` returns the new peer token. Clear-mode setup must retain
finite per-attempt candidate buffers until authenticated selection/transcript
and the appropriate remote description are available, then flush candidates
with the exact token and negotiation generation. #904's final delta sets
128 candidate objects / 64 KiB per pending direction, including existing holds,
and at most one end marker, within the original 15-second setup deadline;
overflow or the existing setup deadline fails that attempt. Do not wait for
ICE connectivity to deliver the answer, drop valid early candidates merely
because the token is not known, or accept tokenless clear-mode candidates.
Fence delayed candidate callbacks to their original attempt. Preserve the
legacy path when the authenticated exchange selects legacy. Test early
trickle in both directions, cancellation, timeout, replacement and both codec
selections. This qualification is now included in the final #904 head.

Preserve chunk limits, ordered delivery, backpressure, epoch fencing, terminal
screen snapshot/cursor recovery and encrypted signaling compatibility fixtures.
Raw binary framing and new terminal replay semantics are separate work unless
the implementation proves they are required for this migration.

Mandatory equivalence does **not** include new periodic WebRTC account
introspection, a 30-second grant expiry, a new signing system or a second
supported authentication profile. The coordinator explicitly superseded that
draft requirement on #904. Existing direct-session availability remains intact.

## HTTP identity, listener and discovery

HTTP is opt-in and independent of WebRTC. Disabled means no listener and no
usable endpoint advertisement. Initial delivery uses native HTTPS with explicit
bind/origin/certificate/key configuration; reverse-proxy termination is deferred
to a separately reviewed configuration. TLS terminates at the configured endpoint;
the endpoint origin is part of the authority contract, never reconstructed from
an attacker-controlled Host/Forwarded header. Cleartext private-network access
and certificate-validation bypasses are not an alternate production mode.

Use separate device-scoped account credentials and online verification because
HTTP bypasses relay authorization. Hosted account credentials must never be
sent to the entered bridge URL. Verification binds issuer/account, approved
device, audience/endpoint generation, scope and expiry. Renewal cannot revive a
closed binding. Revocation, failed renewal and expiry stop new admission and
output by the documented hard deadline; already admitted external effects retain
their actual outcome semantics.

Accept #905's five-minute opaque credential lifetime and hard verification
freshness of at most 30 seconds from introspection request start, capped by token
expiry. Revalidation starts proactively; a monotonic deadline independently
fences admission and output if verification hangs or fails. This deliberately
makes HTTP depend on the hosted verifier within that window. It does not impose
that availability change on WebRTC. Keep family revocation identity separate
from `device_user`; never forward the bearer or credential material to handlers.

Discovery must authenticate **which endpoint** owns the device, not merely
prove that some reachable device can sign a nonce. A forwarded proof from a
victim bridge must fail because its configured canonical origin differs from
the entered origin and the account's current registered endpoint. Bind one-use
challenges to the authenticated account and intended device/origin, expire them,
and consume them atomically. Never adopt a key or endpoint solely from the
untrusted discovery response. The hosted service does not fetch arbitrary user
URLs. The browser uses redirect rejection, omits ambient credentials and sends
no access token until endpoint verification succeeds.

Device endpoint registration must itself prove paired-device ownership and use
the device's configured origin. Endpoint change/disable and account/device
revocation invalidate the relevant grants; stale persisted URLs remain hints,
not independent authority. The TLS terminator can see application plaintext,
which is inherent to the selected HTTPS endpoint trust model and must be stated
in operator documentation.

Use maintained HTTP/TLS/SSE components rather than custom HTTP or crypto:
Axum on Tokio, tower-http for explicit CORS/body policy, rustls/tokio-rustls for
TLS, the existing reqwest client for hosted verification, and existing Ed25519
identity primitives for device proofs. Exact dependency versions/features and
lockfile changes require implementation review; this is not an instruction to
blindly install latest versions. Axum provides an SSE response/keepalive API,
and tokio-rustls integrates TLS streams with Tokio.
([Axum SSE](https://docs.rs/axum/latest/axum/response/sse/index.html),
[tower-http CORS](https://docs.rs/tower-http/latest/tower_http/cors/index.html),
[tokio-rustls](https://docs.rs/tokio-rustls/latest/tokio_rustls/))

For the HTTP device assertions, use standard compact Ed25519 JWS with fixed
algorithm, issuer/audience/type validation and bounded one-use `jti` handling,
as #905 specifies. `jsonwebtoken` (Rust) and PyJWT (hosted Python) are the
proposed JOSE libraries; #910 must pin compatible reviewed versions/features.
Use the existing paired identity key trust, not a key supplied by the proof.

## SSE, recovery and browser constraints

Current SSE carries transient change notifications and separately classified
ephemeral streams. Reconnect uses existing current-state refresh/reattachment.
There are no durable event IDs or no-gap replay claims for ChangeBus. A slow
consumer gets bounded buffering and explicit closure/reset semantics, never
silent queue eviction presented as complete delivery. Heartbeats do not advance
state, refresh authorization or create cursors. Terminal input is not replayed
after uncertain delivery.

Freeze #905's current SSE limits: 15-second heartbeat; 128 queued events and
1 MiB queued bytes including in-flight output; 1 MiB maximum event and
10-second stalled-write timeout. Overflow attempts an explicit reset then
closes. No SSE `id:` is emitted; `Last-Event-ID` is rejected as unsupported.
The hello explicitly reports no durable replay. Reconnect refreshes authoritative
current state before treating the cache as caught up.

#887/#892 own the future journal, incarnation, scoped cursor, snapshot watermark
and retention/reset protocol. #893 owns cache/cursor atomic application. Their
API names are not frozen here. Durable SSE support may be enabled only once
their actual negotiated contract exists. The approved plan and recovery
comments are `tc-01M5WC725H9WG86KQNDZH008JX` and
`tc-01M5WDTY9K8288DND6RQX08DTN` on #887. Coordinate shared dispatch/API-version
changes with that owner before editing overlapping files.

Use fetch streaming with an explicit Authorization header and abort lifetime.
An EventSource object's automatic last-event-ID tracks received events, which
is insufficient for a future cursor committed atomically with local cache
changes. Fetch also provides explicit redirect and credential modes.
([HTML SSE standard](https://html.spec.whatwg.org/dev/server-sent-events.html),
[Fetch standard](https://fetch.spec.whatwg.org/))

An HTTPS page cannot promise arbitrary bridge URLs will work in every browser.
Require browser-trusted certificates and permitted CORS origins; test the
actual hosted CSP and Electron header policy. Local-network access may also
require browser permission, separate from CORS. Do not disable Electron web
security or certificate checks to make discovery pass. Chrome documents
local/loopback permission behavior and recommends explicit rejection handling;
other browser/platform support remains a measured compatibility question.
([Chrome local network access](https://developer.chrome.com/docs/modern-web-guidance/guides/security/local-network-access))

For browser Add by URL, store a bounded account-bound pending canonical origin
without fetching it, then reload an authenticated private/no-store document
whose CSP includes only that candidate and bounded selected registered origins.
Never widen the global policy to `connect-src https:`. Permission to attempt a
connection is not device authority. An already loaded CSP cannot be retracted;
token expiry and closed handles, not CSP replacement, enforce revocation.

For desktop, use typed main-process operations on verified endpoint handles,
with per-device tokens held in memory and outputs bound to the requesting
webContents generation. Keep hosted OAuth header injection restricted. Do not
expose a generic native fetch proxy, URL/header overrides, or renderer access to
device identity keys.

An automation-only REST subset cannot enable the Build connection selector.
Before advertising it, cover the actual nonterminal SPA consumer methods,
including legacy session-scoped calls and background reads. Terminal remains a
separately authenticated WebRTC path in this delivery, explicitly represented
in capability/availability UI. HTTP views remain usable when that terminal path
cannot connect. Do not imply complete HTTP terminal parity or replay input.

## Implementation acceptance and handoff

The implementation remains gated on the coordinator's contract freeze and
file ownership assignment. Architecture approval does not replace Terra review
of each implementation or the independent integration review. Tests must use
the real adapter entry points and current security contract, not only mocks.

| Task | Exclusive responsibility / handoff |
| --- | --- |
| #908 | Shared execution, sessions, output and existing scheduler extraction; minimal initial lib/main/carrier wiring. Preserve legacy parity first. |
| #909 | WebRTC provenance, peer/channel binding and clear/legacy codecs; carrier edits only after #908 hands off its reviewed extraction. |
| #910 | Hosted account grants, registration/discovery, bridge verifier and per-document CSP. Own Python policy/controller changes; #912 consumes their contract. |
| #911 | HTTP listener/runtime, lifecycle, sessions/SSE and limits. After #908's wiring handoff, sole owner of consolidated Cargo/main/lib HTTP/auth integration. |
| #915 | `http/routes/`, explicit resource manifest/OpenAPI and nonterminal consumer coverage; supplies the route contract to #912. |
| #912 | SPA Add by URL, fetch/SSE adapter consuming #915's manifest, and cache/epoch fences; no duplicate Python policy implementation. |
| #914 | Desktop typed main-process adapter, credential isolation and IPC/stream fences. |
| #913 / coordinator | Integration, final capability/version allocation, conflicts, shared-file handoffs and cross-transport acceptance; coordinate API/dispatch overlap with #887. |

New modules and tests may be prepared in parallel after the gate. Shared imports
land in order: #908 extraction/session boundary; #910 verifier contract and #911
runtime wiring; #909 codec and #912/#914 consumers on those exact interfaces;
then #913 qualification. #909 does not depend on completion of HTTP. Each shared
file has one named owner at a time; handoffs name a reviewed base/head, never
rely on concurrent edits in original sources.

The coordinator's seven-slice ownership above supersedes proposal role placeholders.
#915's routes can proceed against frozen contracts alongside #911's runtime;
only #911 lands their shared imports/wiring. #910 supplies dependency and verifier
integration requests to that owner instead of editing the same Cargo/main/lib
files concurrently. Hosted CSP stays with #910, desktop with #914.

Required evidence includes cross-account and forwarded-discovery rejection;
pre-bind, wrong-channel and stale-opening injection; substituted SDP/DTLS
fingerprints; all old/new codec combinations; app/term coexistence; HTTP auth
expiry/revocation and redirect handling; bounded SSE overflow and refresh;
shared admission under load; update drain; and cancellation/uncertain-write
behavior. Durable duplicate-effect tests apply only to enabled semantic verbs.

Capture exact implementation base/head and targeted gate exit codes under
`nice -n 10`, plus semgrep before ready and gitleaks before any authorized push.
Do not widen the existing complexity ratchets. No production edits, live bridge
changes, merges to original sources, deployments or pushes are part of #906.

## Review evidence and limits

This review checked the exact proposal contents and saved snapshot heads against
the base code paths listed above, including relay ownership validation, native
DTLS fingerprint comparison, encrypted-session carrier admission, terminal
key/session multiplexing, scheduler/output lifetimes, mutation owner identity,
current CSP and desktop authorization policy. The #907 matrix was read and
qualified to this final scope. Upstream sync ownership was checked against the
two #887 task comments cited above.

The #904 author reports passing baseline carrier (130), RTC (212), transport
(16), relay (36), SPA (236) and JS/Python transport (8) tests. The #905 author
reports OpenAPI and 27 Gherkin scenario validation, plus security scans. These
are supporting author evidence; they are not independent runs by #906 and do
not test the proposed new behavior. #906 makes no runtime/performance or completed
security-checklist claim. Its own change is only this review document; targeted
whitespace and Semgrep results are recorded on the task with the final commit.
