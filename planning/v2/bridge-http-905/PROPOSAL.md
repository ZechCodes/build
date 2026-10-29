# Account-authenticated REST/SSE and URL discovery

Status: proposed contract for #902/#905 and independent #906 architecture review.
No production implementation or deployment is authorized by this document.
Base: fetched `origin/dev` at `12b8f869bd94e259666df69f1a8c0cde5c695982`.
Branch: `task/905-http-contracts`. Date: 2026-10-29.

## Decisions

Enable an optional HTTPS listener on the user's bridge alongside WebRTC.
Authenticate through the user's existing Build account and current ownership of
the paired device. Issue a distinct, revocable device/client credential family
with short-lived opaque access tokens. A LAN address, certificate, device UUID,
pairing code, gateway token, or successful discovery is insufficient authority.

Use normal TLS, standard JWS device identity proofs, OAuth bearer transport and
authenticated token introspection. Do not add application payload encryption to
HTTP. HTTP bearer tokens are replayable if stolen; discovery proof is endpoint
identity verification, not proof of possession of an access token. DPoP is
deferred hardening, not an implicit promise of this design.

Share typed execution, lanes, operation receipts and domain results with WebRTC
through #903. Share durable sync with #887. SSE is a delivery adapter, not a new
journal. Disablement of HTTP must leave WebRTC usable. No automatic downgrade to
unauthenticated HTTP, and no automatic replay of uncertain mutations on another
transport.

## Repository evidence and gaps

Paths below describe the exact base, not promised implementation:

| Existing source | Finding and consequence |
| --- | --- |
| `bridge/src/config.rs`, `main.rs` | Environment-resolved daemon configuration, explicit dev/prod state isolation; no HTTP application listener. Add one supervised listener without another AppState or startup path. |
| `bridge/src/identity.rs`, `pairing.rs` | Persisted approved device UUID, Ed25519 identity key and approving API origin. Reuse the paired identity for proof; do not export it to clients or provision a shared internal secret. |
| `skriftapp/buildapp/devices_controller.py` | Devices have current account ownership; gateway tokens identify an account to the relay. Gateway tokens have no target device/endpoint audience and must not authenticate bridge HTTP. |
| `skriftapp/buildapp/ephemeral_tokens.py` | Purpose-separated random tokens stored only by hash. Reuse entropy/hash/expiry conventions, but its current record lacks device, family, endpoint and revocation binding: a distinct model/migration is necessary. |
| `skriftapp/buildapp/desktop_auth.py`, `session_auth.py` | Hosted routes authenticate browser session or desktop OAuth identity and require alpha membership. Keep these guards; never send those credentials to an entered URL. |
| `skriftapp/buildapp/internal_auth.py` | Shared secret belongs to relay/internal service calls. Do not copy it to user bridges or expose internal routes to implement introspection. |
| `bridge/src/carrier.rs`, `app/rpc.rs`, `api/v1/` | Lane selection, typed normalization, published reads and receipts already exist; #903 must extract these rather than HTTP invoking app handlers under its own lock. |
| `bridge/src/changes.rs` | ChangeBus coalesces and bounds ephemeral notifications; revision is not a durable reconnect cursor. Terminal output has a separate protocol. |
| `bridge/src/api/writes.rs`, `api/v1/writes.rs`, `store/operations.rs` | #739 operation admission/outcomes already exist. An HTTP 202 must refer to that admission, never a second job database. The base ACK_CONFIRM_METHODS list is empty; route migration must prove actual admission coverage rather than treating all writes as journaled. |
| `spa/src/core/sessionRpc.js`, `bridgeEpoch.js`, `deviceContexts.js` | Existing pending-call, epoch and cache fences must also apply to HTTP results; view rendering stays cache based. |
| `skriftapp/app.yaml`, `spa/README.md` | Current CSP connect-src allows self/data/relay, so arbitrary direct HTTPS fetch is currently blocked. Add by URL requires the explicit document-policy change below. |
| `desktop/src/security-policy.mjs` | Current all-URL header hook strips supplied Authorization and injects desktop OAuth only for hosted API. Renderer direct HTTP would lose its device token. Use a narrowly scoped main-process adapter, not a wider OAuth allowlist. |

Read for intent: `ARCHITECTURE.md` topology, RPC/push, wire capabilities,
relay/direct connection, cache rendering and connection lifecycle;
`E2EE Platform Scope.md` and `UI Design Brief for E2EE Platform.md`.
Older E2EE-only/relay prose does not describe this proposed opt-in TLS path.

## Listener and lifecycle

Proposed startup configuration (names are reserved by this proposal):

| Setting | Contract |
| --- | --- |
| `BRIDGE_HTTP_ENABLED` | Default false. True is explicit operator consent; device approval alone never opens a port. |
| `BRIDGE_HTTP_BIND` | Required when enabled; explicit IP:port, no wildcard default. Wildcard requires an explicit value. |
| `BRIDGE_HTTP_ORIGIN` | Required canonical external HTTPS origin, root path only. It is the sole endpoint authority, independent of Host/Forwarded headers. |
| `BRIDGE_HTTP_TLS_CERT`, `BRIDGE_HTTP_TLS_KEY` | Required native TLS chain/key files. Key owner-only; validate before binding. Use existing rustls provider. |
| `BRIDGE_HTTP_ALLOWED_ORIGINS` | Exact hosted application origin(s), default only the configured approving web origin; no wildcard, null origin, suffix matching or reflected origins. |

The first implementation supports native TLS, one advertised origin and no
untrusted reverse-proxy headers. Reverse-proxy TLS termination is a separate
reviewed deployment profile: it would place plaintext application data and
bearers at the terminator and needs a protected backend socket. It is not enabled
by treating an arbitrary X-Forwarded-Proto as HTTPS. No plaintext exception,
including localhost, in the released account-authenticated profile. Test servers
use fixture certificates and isolated configuration; never change live settings.

Validate scheme/authority, certificate/key, approving API/channel and state paths
before starting an enabled listener. Invalid enabled config is a startup error,
not silent exposure or silent disablement. Initialize shared execution/read
workers before accepting either transport. Bind TLS, complete authenticated
endpoint registration, then admit protected HTTP requests. Registration failure
leaves the listener in discovery-only/not-ready mode and retries with bounded
backoff; existing WebRTC startup remains independent. No credentials are minted
until registration succeeds. Log readiness and fixed error categories without
tokens, proofs, request bodies or private addresses.

On listener failure: release HTTP openings/stream targets and report HTTP
unavailable; retain shared services and WebRTC. On shutdown: stop acceptance,
fence outputs immediately, release HTTP bindings, allow up to 5 seconds for
socket teardown, and let accepted durable operations settle independently.
Configuration changes require restart initially. Persist an endpoint epoch and
rotate it on origin, enablement or security-policy changes; restart alone does
not change it. HTTP disablement closes local sockets immediately and deactivates
the hosted registration when reachable. A crash leaves a stale registry entry,
not an active listener or successful reachability claim.

## Endpoint registration and Add by URL

An origin is parsed by a URL library and represented by HTTPS scheme, IDNA ASCII
lowercase host and effective port; remove only the default 443 port and root
slash. Reject userinfo, non-root paths, fragments, queries, invalid ports,
ambiguous IP encodings, IPv6 zone IDs and trailing-dot aliases. IPv6 literals use
brackets. Compare origins, not string prefixes; keep dev/prod issuers separate.
No DNS lookup is needed to normalize or store an origin. Absolute API paths are
fixed by the contract; discovery cannot supply an alternate credential URL.

### Trusted registration provenance

1. An approved bridge requests a rate-limited registration nonce from its fixed
   approving API: `POST /api/devices/{device}/http-endpoint/challenge`. The nonce
   is random, 60-second, one-use, bound to device and registration purpose.
   Requesting one neither approves a device nor reveals ownership.
2. Bridge submits `POST /api/devices/{device}/http-endpoint` with a standard
   compact JWS signed by its existing Ed25519 key. Its signed claims contain
   `iss=sub=device_id`, exact hosted endpoint `aud`, `iat`, `exp` (<=30 seconds),
   unique `jti`, registration nonce, configured canonical `origin`, endpoint
   epoch and enabled state. No unsigned body field carries registration state.
3. Hosted service selects the public key from its paired-device record, checks
   approved/current ownership, time/purpose/audience, consumes nonce+jti in the
   same transaction as registry update, and stores origin/epoch/identity key
   fingerprint plus owner authorization epoch. Old proofs cannot replace a newer
   registration. A retry obtains a new challenge; a conflicting registration
   must explicitly name its predecessor epoch. Never trust a key embedded in JWS.

This proves the paired bridge intentionally declared that origin; it does not
prove network reachability or certificate validity. Clients independently verify
TLS and live endpoint identity. No hosted URL probe, DNS resolution, HTTP proxy,
certificate issuance, firewall opening or public exposure occurs automatically.

### Browser sequence

1. Signed-in user chooses **Add by URL** and enters origin O. Store normalized O
   as a pending account discovery using guarded
   `POST /api/bridge-discovery/challenges`. Return random challenge ID+nonce,
   expiry (60 seconds), expected approving issuer, and an exact hosted resume
   route. Cap pending challenges per account (8) and input (2 KiB).
2. Current CSP blocks this new origin. Navigate to a fresh same-origin app
   document using the non-secret pending challenge ID. The authenticated server
   derives its response CSP connect-src from fixed dependencies plus O and this
   account's registered selected endpoints (maximum 16). Never interpolate raw
   header text or broaden connect-src to `https:`. The challenge is not an OAuth
   credential and cannot be used without the initiating account. Existing cache
   survives reload; keep the draft. Response is private/no-store and varies by
   authenticated session. This is a deliberate hosted config/controller/test
   change, not something CORS can solve. If another selected endpoint exceeds
   the cap, reopen a document with a new bounded selection.
   Feasibility: the existing authenticated `BuildController._render_spa` already
   returns a per-account no-store Response. Integrate dynamic policy at the
   security-header assembly boundary so exactly one enforced CSP reaches the
   browser; adding a second relaxed header cannot override the static stricter
   policy. Verify the actual middleware ordering and effective response policy
   in an isolated hosted integration test before treating this prerequisite as
   implemented. Preserve every unrelated CSP directive and default policy on
   non-app routes. No framework fork or global source-list relaxation is assumed.
3. Browser directly sends `POST O/.well-known/build-bridge` with challenge ID
   and nonce, `credentials: omit`, `redirect: error`, no Authorization, no
   account cookie, `Referrer-Policy: no-referrer`, 5-second deadline. A public
   static GET descriptor may advertise protocol/version but supplies no trust.
   Reject redirects even on the same origin. Cap proof response at 16 KiB.
4. Bridge signs a JWT/JWS of explicit type `build-http-discovery+jwt` containing
   its device ID, configured O, endpoint epoch, approving issuer, challenge ID,
   nonce, audience = fixed hosted discovery verification URL, `iat`, `exp`
   <=30 seconds and `jti`. It never signs a caller-supplied origin, Host,
   Forwarded value, arbitrary message, claims object or alternative audience.
   Rate-limit this public operation (10/min/source, 60/min/device), bound all
   fields and sign off the app lock. It returns no account ID or application data.
5. Browser submits only the received proof and pending challenge ID to guarded
   `POST /api/bridge-discovery/complete` on its hosted origin. In one transaction,
   hosted verifies stored public key, current owner = authenticated user,
   registration enabled, signed O = pending O = registered O, endpoint epoch,
   issuer/channel, audience/type/time, and exact unspent challenge/nonce. It
   consumes the challenge and creates a device credential family + first access
   token. Mismatch or another owner's device yields a generic refusal and no
   credential. The hosted service never fetches O or URLs in the proof.
6. Persist only device ID/origin/epoch/family handle as account device metadata;
   show device identity before completing connection. Keep access token in
   browser memory. Open an HTTP session, read its authenticated greeting and
   capabilities, then use cache/epoch fences for sync. URL discovery selects an
   already paired account-owned bridge; it does not bypass pairing. An unpaired
   bridge is refused and the existing pairing flow is offered separately.

Forwarding argument: attacker endpoint E forwards the nonce to victim bridge V.
V signs its own configured origin V, while the account pending origin is E.
Hosted comparison fails. Rewriting origin/device/epoch breaks the JWS signature.
Claiming a victim device with an attacker key fails paired-key selection. Reusing
a proof fails challenge account/purpose/expiry/consumption checks. The protocol
does not prevent relaying byte-for-byte at the *same* authorized TLS origin;
normal TLS/DNS/certificate security and the origin operator are trusted there.
A compromised paired identity or trusted TLS terminator remains powerful; no
claim of TLS channel binding or hardware attestation is made.

### Desktop and browser constraints

Desktop main owns discovery and HTTP network requests. Renderer requests carry
an approved connection handle plus typed operation ID/parameters; main resolves
the stored origin itself. New explicit IPC permits only the trusted hosted
frame, authenticated account and allowlisted methods/paths. Bound JSON bodies,
stream bytes, cancellation and event ownership to that webContents generation.
Never expose a generic fetch proxy or URL/header override. Native networking
disables redirects, validates certificates and exact origin, and never attaches
hosted OAuth to device endpoints. It holds device tokens in memory, not renderer
localStorage. Do not widen the existing renderer authorization injection policy.
Sign-out closes all handles, aborts requests and clears device credentials.

Browser requires trusted HTTPS certificates, network reachability, matching CSP,
CORS and any browser/OS local-network permission. CORS does not authenticate
non-browser clients. A self-signed LAN certificate fails fetch until it is
properly trusted; no in-app ignore-certificate option. Chromium's current Local
Network Access permissions supersede the earlier PNA-preflight experiment;
behavior differs by browser/version/OS. Do not promise that a legacy
Access-Control-Allow-Private-Network header makes every LAN URL work. Released
profile requires HTTPS even if one browser relaxes mixed-content rules. Denied
permission, certificate errors and unreachable origins produce a bounded
connection failure and leave cached views intact. WebRTC remains available.

## Credential and authorization contract

Hosted credential record: hashed random secret (at least existing 192-bit
entropy), purpose `bridge-http`, token ID, stable family ID, account ID, device
ID, exact origin, endpoint epoch, owner/device authorization epoch, allowed
scope, expiry, revoked_at and issuing account-session/client relationship.
Access-token lifetime is 5 minutes; never issue a cross-device token or expose
the raw secret after its mint response. Families are revocable separately.
Token family is stable across rotation; token ID changes on each mint and is
audit-only. Tokens are sent only as `Authorization: Bearer` over verified TLS,
never in URL, cookies, redirect targets, logs, crash reports or persistent cache.

Hosted refresh: guarded `POST /api/devices/{device}/http-credentials` takes the
family handle and expected endpoint epoch. Recheck issuing session/client,
current account/device ownership, alpha gate, registration and family state.
Mint a new 5-minute token, with no bridge-held account refresh secret. Family
handle alone authenticates nobody. Browser account logout revokes that session's
families; desktop logout revokes its installation's families and clears native
storage. Device revoke clears approval, increments its authorization epoch and
revokes all families/registration atomically. Account suspension/ownership
change also invalidates grants. Local HTTP disablement needs no hosted roundtrip
to stop local execution.

Bridge introspection: `POST /api/bridge-http/introspect` on the fixed approving
API, TLS and redirects disabled. Body uses RFC 7662 token/introspection fields
plus target device and endpoint binding; it is not an `/internal/*` request.
Authenticate bridge as an OAuth confidential client using an RFC 7523-style
`client_assertion` JWS: iss=sub=device ID, exact introspection URL audience,
explicit separate assertion type, iat/exp window <=30 seconds and random jti.
Hosted checks the paired Ed25519 key, approved ownership and an atomic unique
(device,jti) replay cache through exp+allowed skew (5 seconds). Bound cache,
rate-limit per device, refuse when full rather than evicting live replay guards.
Registration/discovery/assertion JWT types have disjoint validators; reject none,
algorithm substitution, unsupported crit and all remote key URLs.

Only the target device may introspect its token. Hosted rechecks current
account membership, family and endpoint/authorization epochs on every lookup.
Return `active: false` for unknown/revoked/wrong-purpose/wrong-target credentials;
active response includes the full verified binding and scope. Never return a
raw token. Do not trust user/device/epoch claims supplied by the HTTP caller.

**Availability and time:** positive results create a bounded local grant, cached
by token digest + complete target binding. Hard deadline is the earlier of token
expiry and introspection *request start* +30 seconds, using monotonic time plus
the validated remaining token lifetime. Delayed responses never extend this
window; reject one arriving after its deadline. Check every request before
admission and every output send. Deduplicate simultaneous introspections for
one token; cap cache at 1,024 grants/device, at most 8 outstanding introspections,
2-second network deadline, bounded waiting queue. Overflow returns 429 without
execution. Eviction causes revalidation, never extends authority.

An SSE/live opening revalidates proactively after 20 seconds and arms an
independent hard-deadline timer. Failure to refresh cannot extend authority:
existing cached grant remains usable only until its existing deadline; new
uncached requests fail 503 `auth_unavailable`. Expired/revoked credentials return
401 with Bearer challenge. At deadline or observed revocation: cancel opening,
discard queued outputs, release subscriptions and close streams without waiting
for network I/O. Thus online revocation has a maximum 30-second cached window,
not instantaneous revocation; partitions fail closed at that same deadline.
No offline authentication bypass is offered. Apply a final grant/generation
check before delayed replies, not merely at request ingress. Already accepted
durable commands can complete; suppressing a reply is not rolling back a write.

Only the verifier constructs #903's opaque `VerifiedCaller`. Hand it to
`ClientSessions::open(VerifiedCaller, SessionOptions)` and
`ExecutionService::submit(RequestContext, Invocation<M>) -> Call<M::Reply>`.
Session binding includes stable family/account/device/authorization epoch and
endpoint epoch; token refresh must prove identical binding. A different binding
requires reopen. Shared principal resolution maps paired owner to the existing
store-local `device_user` principal, excluding token/family/session IDs so #739
operation retries/status remain valid across transports and rotation.

Per the coordinator's superseding baseline decision, WebRTC retains current
trusted configured relay intake and existing account/device ownership validation.
#904/#909 binds that verified provenance to Opening and actual SDP/DTLS peer with
a finite setup/bind timeout. No new mandatory WebRTC introspection, token refresh
or 30-second authorization expiry is introduced. Additional RTC online-account
hardening is out of scope, not a second supported profile. #903 therefore stores
distinct trusted proof/lifetime variants internally behind opaque VerifiedCaller;
HTTP fields/freshness are not compulsory getters for relay callers. HTTP tokens
can never authorize WebRTC. Shared stable store principal still spans both paths.

## REST and OpenAPI

The HTTP adapter is a REST resource surface over the same typed operations;
OpenAPI 3.1 describes HTTP lifecycle/auth/errors and concrete typed request/result
schemas. [openapi.yaml](openapi.yaml) is a deliberately bounded initial contract
skeleton, not a claim of current server support or full domain schema generation.

| HTTP route | Shared behavior |
| --- | --- |
| `POST /v1/sessions` | Verified caller opens logical client session, returns session ID, opening generation, greeting (wire version/capabilities/bridge epoch) and hard grant expiry. |
| `PATCH /v1/sessions/{session}` | Refresh using new verified token of same family/binding; do not mutate principal from body fields. |
| `DELETE /v1/sessions/{session}` | Fence only this opening/targets; no domain write rollback. |
| `GET /v1/projects`, `/v1/workspaces`, `/v1/agents` | Existing typed directory reads, same account/device scope. |
| `GET /v1/projects/{project}/tasks` | Opt-in bounded summary read; same normalized filters/cursor/default100/max500/1MiB result as #890 and later #900. |
| `GET /v1/projects/{project}/tasks/{task}` | Current typed detail; expose bounded detail separately only when #901 capability exists. Never silently reshape legacy detail into summary. |
| `POST /v1/projects/{project}/tasks` | `tasks.create`, typed params; journal metadata where supported. |
| `PATCH /v1/projects/{project}/tasks/{task}` | `tasks.update` with existing body/version preconditions. |
| `POST /v1/projects/{project}/tasks/{task}/comments` | `tasks.comment`, same mention/attachment/ref semantics. |
| `GET /v1/operations/{operation}` | #739 authoritative receipt/outcome status for verified stable principal. |
| `POST /v1/subscriptions` | Create explicit authorized bounded subscription, session+generation bound. |
| `GET /v1/subscriptions/{subscription}/events` | Fetch-stream SSE, independent output binding. |
| `DELETE /v1/subscriptions/{subscription}` | Release stream; leave durable commands untouched. |

These routes are the first implementation slice, not sufficient acceptance for
the user-visible Build connection. [client-coverage.md](client-coverage.md)
inventories additional interactive resource routes required before Add by URL
advertises a usable REST/SSE connection. Staged subsets may be tested as
automation APIs, but do not enable the Build connection selector or imply views
work. Final delivery must cover all advertised nonterminal views and lifecycle
operations, or explicitly withhold that capability/view before selection.
Terminal may retain an explicit separately authenticated WebRTC path with clear
availability/capability UX. No generic method-name tunnel to legacy app dispatch,
arbitrary `rtc.*` or QA endpoints. Generate
per-route operationId/parameters/result schemas from the same typed registry
and route manifest; fail build if an advertised method lacks a contract fixture.
Do not write another business model/OpenAPI schema by hand. A partial HTTP
capability cannot cause the SPA to route unsupported calls to it. Expose separate
automation/read and usable-client capabilities; select the latter only after
route coverage and end-to-end acceptance, with an explicit terminal policy.

Authentication is checked before resource lookup. All application requests bind
`X-Build-Session` to that verified owner/family/device and opening generation;
session/subscription IDs are unguessable handles, never authority by themselves.
Path project/task identifiers normalize into the same typed selectors and
ownership checks as existing calls. Method/body/path disagreement is 400.
Reads enter #903's published/SQL/I/O lanes; excess queued work follows the same
fair bounded admission, never holds the app lock while waiting for HTTP writes.

Success reads return the typed result. Mutations return 200/201 only for a known
final result; journaled writes return 202 only *after persisted admission*, with
the original operation ID/receipt and status Location. `Idempotency-Key` is the
same #739 operation ID, not an additional HTTP retry namespace. Preserve write
causal predecessor metadata in `X-Build-After` using its existing typed form.
Do not imply every legacy mutation is journaled. Nonjournaled mutations await
their final result or return uncertain transport failure; never blindly retry.
No accepted HTTP response is sent before shared admission succeeds. Status
reads expose pending/confirmed/failed/unknown accurately after reconnect.

Error body: `{error, error_code, retryable, details?, request_id}`. Preserve
shared ApiError codes/details; HTTP transport failures use a separate namespace
and do not change existing wire retryability. Mapping: malformed/params 400,
auth invalid 401, forbidden scope 403, missing 404, conflict/stale/locked 409,
body too large 413, unsupported version 426, adapter quota 429, temporarily
unavailable/auth authority unavailable 503, unexpected internal 500. Shared lane
`busy` retains its typed retryability. HTTP disconnect/timeout after dispatch is
an uncertain outcome even if no 202 reached the client. A missing capability is
explicit unsupported, never successful empty data. Unknown route is 404.

Adapter bounds: max JSON body 1MiB, header bytes 16KiB, header read 5 seconds,
idle request body 10 seconds, 128 sockets/device, 16 openings/family, maximum
32 concurrent requests/opening. Attachment bytes retain existing chunk API
bounds; reject larger inline payloads before buffering. Domain result bounds
remain method-specific: the 1MiB task-page limit is not imposed on all legacy
detail/file results. Route export requires a documented maximum response or
bounded chunking; exceeding it yields an explicit error, never truncation.

## SSE subscriptions and recovery

Subscription creation body names session, kind and typed selectors, maximum
64KiB. Initial kind `live` adapts existing ChangeBus subscriptions with their
entity/kind normalization; max 8 subscriptions/opening, 512 selected entity keys
and 32 kind selectors. Reject refused/unauthorized kinds before sending headers.
Never put unbounded arbitrary filter JSON or secrets into stream URLs. Server
IDs bind principal, session/opening generation and immutable normalized scope;
changing scope creates a new subscription. One active output binding per
subscription; replacement fences the old binding without ending the opening.

Fetch with Authorization and `X-Build-Session`, credentials omitted and redirects
disabled. Do not use native EventSource, whose constructor lacks arbitrary
Authorization headers and automatically advances its received Last-Event-ID.
Use a bounded UTF-8 SSE parser with AbortController. Response is
`text/event-stream; charset=utf-8`, `Cache-Control: no-store`, with buffering and
compression disabled for streams. Send a comment heartbeat every 15 seconds;
heartbeat does not renew authentication. Max event 1MiB and parser buffer 1MiB
plus a small framing allowance; never split JSON across independent events.

Events: `hello` (current epoch/generation), `changes` (existing typed ChangeBus
payload), `operation` (receipt hint; #739 status remains authoritative), and
`reset_required`/`closing` controls. Initial live stream **does not emit `id:`**.
Its hello declares `durable:false`; Last-Event-ID on it is rejected with 400
`cursor_unsupported`. Reconnect refreshes relevant state using the existing sync
path and epoch fences. A heartbeat and transient revision are not no-gap proof.

Queue at most 128 events AND 1MiB per binding; count the event being sent until
socket handoff completes and bound socket writer buffers. One slow write may
wait 10 seconds, then abort its binding. Overflow never blocks producers or
other clients: coalesce live hints where their semantics allow, else signal
reset_required best effort and close. A dropped control frame is safe because
every reconnect revalidates recovery state. No promise that every intermediate
live event or terminal byte survives. Terminal output is excluded from this
initial stream; it retains its existing separate ACK/reset protocol.

Durable subscription kind is withheld until #887/#892 publishes its capability
and producer coverage. Then wrap the exact journal read/snapshot/head contracts;
no second cursor, log, compactor or snapshot lease. One SSE durable subscription
corresponds to one normalized scope; no accidental cursor acknowledgement across
scopes. Start from the cursor read out of the client's committed cache. Prefer
bounded journal reads triggered by coalescible SSE wakeups; if batches are sent
over SSE, their event ID is #887's opaque scope cursor and carries identical
batch semantics. Never resume from the parser's last received ID. Commit cache
updates/removals/invalidations and cursor in ONE IndexedDB transaction, then
publish to views. Abort/crash before commit => replay; dedupe by authoritative
entity revision and preserve membership removal semantics. #887's owner confirmed
#892 is not dispatched and has no frozen method, trait, capability or error-schema
names or ETA. #893 owns cache integration. `resync_required` here describes the
approved semantic outcome, not a frozen new transport error. #900 query/page
cursors are not sync cursors. Coordinate dispatch/API version edits with the sync
owner before implementation overlaps. Source plan: #887/c/tc-01M5WC725H9WG86KQNDZH008JX;
recovery: #887/c/tc-01M5WDTY9K8288DND6RQX08DTN.

Snapshot plus watermark must be consistent; consume bounded snapshot lease
pages, atomically replace that scope, then replay strictly after its watermark.
Catch up through a captured finite head. Malformed/future/foreign/stale cursors
use #887's exact structured errors/resync rules; do not silently skip to now.
Long absence/retention/overflow resets only affected scope. Surviving ordinary
restart changes process epoch, not durable journal incarnation. Periodic head
check (initially 30 seconds, foreground immediately) repairs lost notifications
even if SSE looks connected. Wakeups may be lost; committed changes cannot be
silently skipped. Reconnect backoff 0.5/1/2/4/8 seconds capped at 30 seconds with
jitter, honoring Retry-After; 401 requests hosted refresh, permission failures
stop automatic retries. Retrying an SSE GET never repeats a domain mutation.

## CORS and credential leakage

Public descriptor/discovery permits only configured exact app origins; no
browser credentials. Protected routes allow those origins and explicit
Authorization, Content-Type, X-Build-Session, Idempotency-Key, X-Build-After and
Last-Event-ID headers. Expose request ID, Location and Retry-After as needed.
OPTIONS validates requested method/header/origin and is bounded; it grants no
session and returns no protected body. No Access-Control-Allow-Credentials.
Set Vary: Origin on origin-dependent responses and no-store on proof/auth/data.
Unknown/missing Origin from native clients still requires bearer authentication;
Origin is never an account identity. Reject unexpected Host/authority on native
TLS listener rather than allowing DNS-rebinding aliases. Preflight permission
cannot replace the origin/identity proof or token audience checks.

Every network layer, including hosted introspection and desktop main, forbids
automatic redirects. The client computes fixed routes from the verified origin;
it cannot follow token-bearing Location/absolute links to a new origin. Hosted
cookie/OAuth never leaves hosted origin, and device token never reaches another
device, relay or app endpoint. Account sign-out, grant expiry and opening changes
abort in-flight streams; late responses fail generation+grant checks before
touching cache. Product UI reports the configured transport and TLS endpoint
trust without continuing the old claim that an arbitrary terminator sees only
ciphertext. Push notification encryption is unchanged by this HTTP design.

## Standards and dependency choices

These are candidates to validate/pin in implementation dependency review, not
unreviewed lockfile changes. Use compatible stable releases older than seven
days where package policy applies; inspect advisories/licenses/provider graph.

| Primitive | Existing/recommended component and reason |
| --- | --- |
| HTTP routing/SSE | `axum` on `hyper`/Tokio with typed extractor/response/SSE support; add explicit body/concurrency/time/byte bounds. Existing relay only uses tungstenite and is not an application HTTP framework. |
| TLS | Existing `rustls` 0.23/aws-lc provider, `tokio-rustls` already a dev dependency; promote compatible server support and PEM parsing with lock review. No OpenSSL dependency through a convenience JOSE stack. |
| Device proofs | Standard compact JWS/JWT, existing Ed25519 key. Candidate Rust `jsonwebtoken` with reviewed provider/EdDSA support, hosted PyJWT over existing `cryptography`. Cross-language RFC vectors before adopting versions; no hand-rolled concatenation/signature/canonical JSON scheme. Browser submits proof to hosted verifier, avoiding a third JS JOSE dependency. |
| Token transport | [RFC 6750](https://www.rfc-editor.org/info/rfc6750/) bearer header over TLS; existing random token/hash convention, extended target-specific model. |
| Introspection/client auth | [RFC 7662](https://www.rfc-editor.org/info/rfc7662/) active-state query with device binding; [RFC 7523](https://www.rfc-editor.org/info/rfc7523/) signed client assertion profile with explicit replay cache. |
| JWT validation | [RFC 8037](https://www.rfc-editor.org/info/rfc8037/) Ed25519 JOSE and [RFC 8725](https://www.rfc-editor.org/info/rfc8725/) explicit algorithm/type/audience validation. |
| Browser streaming | Native fetch/ReadableStream with [HTML SSE framing](https://html.spec.whatwg.org/multipage/server-sent-events.html), bounded parser; no query-string bearer workaround. |
| Browser LAN | [Chromium Local Network Access guidance](https://developer.chrome.com/docs/modern-web-guidance/guides/security/local-network-access), checked 2026-10-29; permission and platform qualification required. |

Library references checked: [axum SSE](https://docs.rs/axum/latest/axum/response/sse/),
[jsonwebtoken](https://docs.rs/jsonwebtoken/latest/jsonwebtoken/),
[PyJWT](https://pyjwt.readthedocs.io/en/stable/usage.html).
Exact package versions and Rust/CPU portability remain implementation review
items; current documentation alone is not a dependency security approval.

## Ownership, sequence and acceptance

No production files change in #905. Ownership below is a proposal to the
coordinator; shared files require one named integrator, not parallel edits.

| Slice owner | Exclusive files or interface responsibility |
| --- | --- |
| #903 execution implementer | `bridge/src/execution/`, shared caller/session/output types, lane extraction and integration touchpoints in `carrier.rs`, `app/rpc.rs`, `lib.rs`; exported typed route metadata. |
| #904 transport implementer | Authenticated WebRTC/DTLS peer binding, signaling protocol/compatibility and clear payload migration; terminal session proof. |
| Hosted auth/discovery implementer | New `skriftapp/buildapp/bridge_http_auth.py`, `bridge_http_discovery.py`, dedicated tests/migration; one integrator owns edits to `models.py`, device revoke/controller routing and security-header config. |
| HTTP adapter implementer | New `bridge/src/http/{mod,config,auth,discovery,routes,sessions,sse,limits}.rs` and focused tests; consumes execution verifier constructor, owns no business handlers/journal. Integrator alone edits main/config/service wiring and Cargo manifests. |
| Client implementer | New `spa/src/core/httpSession*.js`, `httpDiscovery*.js`, `sseReader.js`, Add by URL sheet and tests; one integrator owns api/connection/capability touchpoints. Preserve cache writers/fences. |
| Desktop implementer | `desktop/src/bridge-http.mjs`, narrow preload/main IPC and security tests; keep existing OAuth injection restrictions. |
| #887/#892 | `store/` sync journal, producer hooks, snapshots, scoped cursor/head/read contracts; #893 owns cache cursor transaction/catch-up. HTTP consumes, never recreates. |
| Coordinator | Final wire capabilities/fixtures, ARCHITECTURE and product trust-text integration, dependency/version conflict resolution. |

Implement only after #906 freezes architecture. Suggested dependency order:
(1) hosted endpoint/auth models and verifier, (2) #903 execution and native TLS
listener, (3) explicit resource routes plus typed contract fixtures, (4) live SSE
and client/desktop flow, (5) #887 durable adapter after its capability exists,
(6) remaining route/terminal parity and integration qualification. Each slice
uses TDD and independent Terra review; Astra reviews architecture/integration.
Do not block HTTP live hints on building a competing durable sync system.

[security.feature](security.feature) is the normative acceptance test
specification to implement against real isolated components in those slices.
It is not an executable test result, and none of its assertions are claimed
passing today. Public HTTP capability stays off until its implemented scenarios,
wire fixtures, cross-transport retry tests and relevant security checklist are
100/100. No placeholder checked boxes or code-presence tests count as security
evidence. Capture before/after admission latency, bytes, bounded memory under a
slow SSE consumer and unrelated-client latency with two simultaneous transports;
no performance claim follows from this design-only task.

The #903 boundary names align with its committed proposal at
`60fcb19d8f359c0d5f3dc728d4afb44305ece7c0` (`docs/shared-execution-contracts.md`):
ClientSessions, SessionLease, privately minted RequestContext, ReplyLease,
EventBinding and SessionEvents. This is a coordinated interface reference, not
a claim that #903 production code is present at this task's base.

Review decisions needed: accept the 30-second cached authorization/online
dependency contract; accept per-document exact-origin CSP for browser discovery;
freeze initial REST resource scope and separately track full SPA/terminal parity;
select reviewed dependency versions. No request for live rollout or shared-branch
merge is made here.

Candidate CSP permission is only permission to attempt a network connection.
It creates no device authority. Pending origins expire after 60 seconds and
disappear on the next authenticated document response. Remove disabled/revoked
origins from subsequent documents immediately; a loaded page's CSP cannot be
retracted. Therefore revocation closes local handles, clears tokens and triggers
reload, and server authorization expiry protects against a suspended page that
misses that signal. A stale document may still attempt a credential-free network
request to a previously permitted origin; it cannot obtain a fresh credential.
This residual reachability is stated explicitly. Default desktop permissions
stay notifications-only; main-process networking still obeys OS permission and
certificate checks. No global browser permission or certificate bypass is added.
