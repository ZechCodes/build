# Account-bound HTTP authorization implementation — #910

Base: `fd2341443e0ffb88cdad7e4c9bb73ff53eb16269` (reviewed contracts), with source
base `12b8f869bd94e259666df69f1a8c0cde5c695982`. The controlling architecture is
[the frozen review](../Bridge%20Dual%20Transport%20Architecture%20Review.md), plus
the #916 desktop parent-credential qualification, #922 authenticated allocation
disposition and #923 login transaction ownership disposition. This document describes the
owned authority modules, not listener/client readiness or deployment approval.

## Integration boundary

`http::discovery::CanonicalOrigin::parse` accepts only canonicalizable HTTPS
origins; no DNS lookup occurs. Root paths/default ports normalize away. Userinfo,
query/fragment, controls, backslashes, percent-encoded authorities, trailing dots,
ambiguous IPv4 and invalid DNS labels are refused. Unicode domains use IDNA ASCII.

`DeviceProofSigner::new(&StoredIdentity, issuer, origin, endpoint_epoch)` requires
an approved paired identity and its configured approving issuer. The default
issuer applies only to identities written before `approved_by` existed. The key
never enters Debug output. It exposes fixed-purpose signing methods:

- `registration_challenge_assertion(now)` signs only the fixed paired issuer's
  challenge audience; it authorizes nonce allocation, not endpoint changes.
- `discovery_proof(challenge_id, nonce, now)` signs configured origin/epoch/issuer,
  device identity, and the fixed hosted completion audience.
- `registration_proof(challenge_id, nonce, previous_epoch, enabled, now)` signs
  the exact predecessor and complete endpoint registration state.
- `introspection_assertion(token_hash, now)` signs the SHA-256 bearer digest and
  fixed introspection audience. It cannot sign caller-supplied claims/audiences.

All four assertions use EdDSA compact JWS, distinct explicit `typ`, random `jti`,
a declared lifetime of at most 30 seconds and the existing paired Ed25519 seed.
The hosted JOSE validator explicitly allows at most five seconds clock skew;
replay records remain through `exp+5`, with no widened tolerance. The hosted verifier
selects the stored paired public key; it rejects extra JOSE header fields,
algorithm changes and wrong purpose/audience/time. Replay records are consumed
atomically with authority changes and retained through expiry plus five-second
skew. Registration/discovery challenges last 60 seconds and spend conditionally.

`EndpointRegistrar::new(Arc<DeviceProofSigner>)` uses HTTPS only, disables
redirects, bounds each request to two seconds and response to 16 KiB, and validates
the echoed registration tuple. `register(previous_epoch, enabled)` implements
both registration and disablement. #911 owns persisted epoch metadata, listener
readiness, retries, rate limits, native TLS and local shutdown; these modules do
not open a listener or advertise a capability.

`HttpAuthVerifier::new(Arc<DeviceProofSigner>)` privately mints
`VerifiedHttpGrant`. Its immutable `HttpGrantBinding` exposes issuer, account,
device, family, origin, endpoint epoch, device authorization epoch and scopes.
It exposes no bearer or rotating token ID. The durable principal is `device_user`.
`verify(token)` and `refresh(grant, token)` introspect only the fixed issuer;
renewal requires the same complete binding and a live lease. `check()` and
`ensure_scope()` fence admission/output; `close()` cannot be undone by refresh.
The verifier's local close fence also invalidates outstanding and evicted grants.

Positive authority expires at the earlier of token expiry and **introspection
request start** plus 30 seconds, measured by Tokio monotonic time. `refresh_at()`
requests proactive renewal after 20 seconds; #911 owns the independent output
and hard-deadline timers. Network failure cannot extend cached authority.
The verifier coalesces simultaneous introspections, caps cached entries at 1,024,
network calls at eight, and queued/cancelled background lookups at a finite bound.
Only token digests and validated evidence remain in the cache.

#908 owns execution's restricted `VerifiedCaller::verified_http` constructor.
#911 converts a verified binding and captures a grant clone in the execution-owned
liveness callback. Execution independently checks the captured deadline. Neither
execution nor domain handlers import credentials or select their own identity.
A refreshed session must renew its execution lease, not merely replace a token.

## Hosted endpoints consumed by #912 and #914

All returned expiry values are integer Unix seconds. Credential responses are
private/no-store. Authentication is the existing browser session or verified
`build-desktop` OAuth account with current alpha membership and active account.
These credentials are used only on the configured hosted origin.

| Request | JSON response / behavior |
| --- | --- |
| `POST /api/devices/{device}/http-endpoint/challenge` with `{proof}` | `{challenge_id, nonce, expires_at}`; fixed `build-http-registration-challenge+jwt` assertion from the paired key; later endpoint change still needs its separate registration proof |
| `POST /api/devices/{device}/http-endpoint` with `{proof}` | `{origin, endpoint_epoch, enabled}` |
| `POST /api/bridge-discovery/challenges` with `{origin}` | `{challenge_id, nonce, origin, account_id, expires_at, issuer, resume_url}` |
| `GET /api/bridge-discovery/challenges/{id}` | Same pending payload; same authenticated account/client only |
| `POST /api/bridge-discovery/complete` with `{challenge_id, proof}` | Mint response described below |
| `POST /api/devices/{device}/http-credentials` with `{family_id, endpoint_epoch}` | Mint response; same original authenticated client binding required |
| `DELETE /api/devices/{device}/http-credentials/{family}` | 204; revokes that authorized family |
| `POST /api/bridge-http/logout` with `{}` | 204; revokes that authenticated client's families and tombstones its binding |
| `POST /api/bridge-http/introspect` with `{token, client_assertion}` | `{active:false}` or verified fields below; device assertion required |

Mint response: `{issuer, account_id, device_id, origin, endpoint_epoch, family_id,
access_token, expires_at, scope, name, fingerprint}`. Scope is the OAuth-style
space-delimited `bridge:read bridge:write`; read permits the separately scoped SSE
routes. Active introspection adds `active:true`, `authorization_epoch`, `token_id`
and `exp` instead of `expires_at`; it never returns the token.

The discovery resume route is `/app/?bridge_discovery={challenge_id}`. The ID is
not an account credential. The authenticated app document's CSP adds the account's
bounded enabled owned origins and this client's live pending origin. Middleware
keeps exactly one enforced CSP, preserves unrelated directives, and returns
private/no-store with no-referrer. The pending endpoint never becomes authority
through CSP. The hosted service never fetches the candidate URL.

On discovery completion, configured bridge origin must equal pending origin and
current registered origin, with matching endpoint epoch, current account owner,
paired key and challenge. Forwarding a proof from another origin therefore fails.
Endpoint change/disable and device revoke invalidate relevant families. Browser
logout tombstones the old signed-cookie client binding to prevent replayed cookie
state minting fresh authority. Loaded CSP cannot be retracted; authority expiry
and closed client handles enforce revocation, not replacing a policy header.

## Browser login identity and transaction ownership — #923

Browser authority requires an unpredictable active `BridgeHttpClient` identity
established at authentication completion. Missing/pre-binding legacy cookie state
cannot acquire it on an ordinary request; reauthentication is required for bridge
HTTP. A legacy or revoked session can still render the cached app shell with its
baseline CSP, without acquiring bridge API authority or a selected bridge origin.

The supported `after_login` hook classifies the trusted resolved handler callable
against this pinned inventory, not its URL, posted method, object attachment or
`is_new_user` flag:

| Resolved completion | Identity transaction owner |
| --- | --- |
| Build `complete_primary_method_registration`; Skrift `complete_primary_method_auth` | Same attached request AsyncSession; add/flush only; framework commit at `auth.py:657/762` |
| Skrift `oauth_callback`, `complete_second_factor_verification`, `dummy_login_submit`, `verify_email_claim` | Owned private identity transaction, after the framework's existing account commit |

The borrowed path requires the user attachment to equal the configured request
unit of work. It never commits, closes or rolls back that session. User/enrollment
or sign-count changes and the new client identity commit together through the
framework after its later `after_user_created` and `login_redirect` hooks.
Independent observers must see nothing until that normal commit. The owned path
locks and verifies the already persisted user, commits only the identity, and
does not commit unrelated caller work such as dummy role assignment. Detached
persisted users are accepted only on a known owned completion. Pending-factor
transitions mint no browser identity until final authentication promotion.

Unknown/replaced handler contexts, missing/mismatched borrowed sessions, or a
runtime dependency version outside Skrift `0.2.0a10`, Litestar `2.24.0`, Advanced
Alchemy `1.11.0` fail closed. Upgrades require re-auditing the inventory and commit
positions. The pinned wheel SHA-256 is
`265c7486dc49b47fbe37d3e3f6be20bb35e9cd348d21183e08ccf24fb1bae36d`.

`BrowserLoginGuard` is configured through Skrift's existing user-middleware
extension, inside session middleware (`asgi.py:1166`). It covers the known
completion's whole response lifetime, including Build's remaining invite work.
It clears scope session state on escaping failures/cancellation, or before an
error response-start reaches session cookie serialization. It also preserves a
failure marker while Litestar's inner exception handler sends a converted error:
Python's active handled exception remains visible during that awaited send, so
a success-shaped custom error response cannot bypass the guard. No database
transaction is performed by this guard. It never turns errors into success.
The hook refuses to mint if the guard is absent.

Before response-start, failures must issue no authenticated cookie. On borrowed
precommit failure the framework owns rollback/close for account/enrollment/client;
on post-commit paths an account and possibly unused identity row may remain, but
no authenticated response carries that failed identity. There is no compensating
account deletion. A disconnect after a successful commit cannot undo it or recall
already transmitted headers. External passkey credential verification remains
Skrift's responsibility; transaction-seam fixture tests do not establish it.

## Desktop parent credential qualification

Skrift 0.2.0a10's access JWTs expose a verified `jti` and expiry, but omit the
refresh-token `family_id`. No access-to-refresh-family database relationship
exists. Do not invent installation identity or revoke every desktop installation.
HTTP families bind exact verified issuer/account/client/access-jti and the
immutable parent expiry. Family/token expiry cannot exceed that expiry; bridge
tokens additionally last at most five minutes. Introspection rechecks parent
jti revocation and expiry. A new OAuth access jti requires discovery and a new
family. #914 fences old handles on rotation/signout before bounded best-effort
family revocation and credential erasure.

Offline signout or a crash cannot notify hosted state immediately. Existing
bridge tokens end within five minutes (and no later than parent expiry); a holder
of the old OAuth access credential can mint only until its original expiry.
This is access-token-family isolation, not server-enforced installation logout.

## Dependencies and verification

Production Cargo/main/lib changes belong exclusively to #911. A separate locked
`bridge/tests/http_auth_harness` compiles the actual auth/discovery modules with
minimal persisted-identity/config fixtures, so tests can run before that wiring
handoff. The fixture is test-only; network behavior uses the real reqwest client.
Full native TLS/listener and execution integration remain #911/#913 gates.

- `jsonwebtoken =10.3.0`, no default features, `aws_lc_rs`: MIT, Rust 1.85 minimum,
  published 2026-01-27 and not yanked on 2026-10-29. This version fixes the upstream
  [authorization type-confusion advisory](https://github.com/Keats/jsonwebtoken/security/advisories/GHSA-h395-gr6q-cpjc).
  [Source/features](https://docs.rs/crate/jsonwebtoken/10.3.0/source/Cargo.toml).
- `aws-lc-rs =1.17.0`: the version matches the frozen production lock, but the
  feature graph changes. Baseline resolved features are `aws-lc-sys, prebuilt-nasm`;
  the JOSE graph adds `alloc, default, ring-io, ring-sig-verify`. The default
  compatibility verification feature introduces ISC-licensed `untrusted =0.7.1`
  alongside baseline `untrusted =0.9.0`. This is an explicit dependency/feature
  delta requiring independent review, not an unchanged provider graph. Its
  library imports the existing seed/public pair and serializes PKCS#8 for JOSE;
  there is no handwritten ASN.1 or extra crypto provider. The
  [upstream feature documentation](https://github.com/aws/aws-lc-rs/blob/main/aws-lc-rs/README.md)
  confirms the compatibility dependency. Captured `cargo metadata --locked`
  results match both manifests; #911 owns the consolidated production lock.
- Hosted `PyJWT =2.15.1` uses existing cryptography Ed25519 and explicit validators.
  [Release/provenance](https://pypi.org/project/PyJWT/2.15.1/). `idna =3.18` pins the
  already-locked UTS46 implementation for origin equality with Rust URL parsing.
- `reqwest =0.13.4` in the harness matches the frozen production lock and disables
  redirects for credentials/assertions. No insecure certificate mode is added.

Cross-language fixtures are `fixtures/bridge-http/device-proofs.json` (three
profiles) and `fixtures/bridge-http/registration-challenge-proof.json` (the fourth
profile), emitted by the harness examples using a public fixed test seed. Python
runs all four through the real PyJWT validators, including purpose/key/claim
substitution negatives. These fixtures contain no deployed credentials.

Run gates under `nice -n 10` and inspect exit codes:

```sh
nice -n 10 cargo test --locked --manifest-path bridge/tests/http_auth_harness/Cargo.toml
nice -n 10 cargo clippy --locked --manifest-path bridge/tests/http_auth_harness/Cargo.toml --all-targets -- -D warnings -W clippy::cognitive_complexity
nice -n 10 cargo fmt --manifest-path bridge/tests/http_auth_harness/Cargo.toml --check
cd skriftapp
nice -n 10 uv run --frozen python -m pytest buildapp -q
nice -n 10 uv run --frozen ruff check buildapp
```

The final task report records actual test counts, scan exits and exact head.
No deploy, live bridge change, shared-source merge or push is authorized here.

## Authenticated nonce allocation and rollout — #922

UUID-only or unsigned requests cannot consume per-device pending capacity. The
controller bounds the streamed JSON body to 18,000 bytes before copying chunks;
the proof validator additionally limits proof strings to 16,000 characters. Under
one device row lock and transaction, the server validates the current paired
key/approved device/active owner, spends unique `(device,jti)`, checks the unchanged
eight-pending/1,024-replay capacities, and allocates a 60-second nonce. A nested
transaction rolls back proof spend and nonce together on capacity or storage
failure; outer commit failure also leaves neither durable. Live replay guards
and pending nonces are never evicted for room. Final registration retains its
independent proof and predecessor-epoch CAS.

Old registrars sending `{}` fail closed. New registrars rejected by an old hosted
allocator do not fall back. Lost successful responses require a fresh signed
proof on a later bounded/backed-off attempt, leaving the abandoned nonce until
expiry. Both enable and disable use authenticated allocation; local disable
remains immediate even when hosted registration fails. #911 owns runtime backoff
and local listener shutdown, which remain integration gates.

Mixed hosted workers do not provide protection through the old unsigned route.
Every old allocator worker must be drained or routed away before claiming the
change effective. Legacy capacity recovers within at most 60 seconds after the
**last** unsigned allocation, not after the first upgraded worker starts. These
are rollout constraints, not deployment authorization.

The fourth cross-language fixture is stored in RFC7515 flattened JSON form at
`fixtures/bridge-http/registration-challenge-proof.json`; tests reassemble the
standard compact JWS and validate it with PyJWT. It uses the same public fixed
test seed and includes no deployed credentials.

## PostgreSQL author concurrency evidence

`test_bridge_http_postgres.py` uses disposable databases on PostgreSQL 16.15,
independent physical connections, and observed `pg_blocking_pids` relationships
before releasing the first transaction. Twelve cases cover one-use discovery,
unique proof spend, endpoint predecessor CAS, logout/issuance ordering, signed
allocation capacity races and both statement/commit rollback. Raw logs preserve
server version and distinct holder/contender/observer PIDs without credentials,
proofs, tokens or nonces. These are author witnesses, not independent acceptance.

Run against a disposable test-only PostgreSQL service, never a production URL:

```sh
BRIDGE_HTTP_TEST_POSTGRES_URL='<disposable asyncpg URL>' \
  nice -n 10 uv run --frozen --project skriftapp python -m pytest \
  skriftapp/buildapp/test_bridge_http_postgres.py -q -s --disable-warnings
```

The author service was an isolated `postgres:16-alpine` container with no host
mounts, a 256 MiB tmpfs, 512 MiB memory, one CPU and loopback-only dynamic port. Image
ID: `sha256:721873c34ceb9f8d8fc265984940dc982404c105f19ad51be9fdc5970a6080ea`.
Tests create/drop unique temporary databases and preserve no application state.
The opt-in test variable is deliberately separate from application DB settings.
A missing test URL skips these author cases and cannot count as concurrency
evidence; the final task report records a run where all twelve executed.

## Corrected author gate attribution

The final hosted source gate at
`76d9fc1f4ad60572e5e4cd457c429c16a6a9c5e8` exited 0 with 1,169 passed and ten
baseline skips: nine dev-schema/privilege witnesses require their separate dev
and admin URLs, and one landing artifact case requires the landing build. All
twelve #910 PostgreSQL cases executed. The first full aggregate exited 1 solely
on the unchanged shared-database-wiring ratchet; the session constructor moved
into `db_test_support.py`, and the corrected full aggregate then exited 0. The
original failure, focused red/green and corrected full log are preserved as
task attachments. No ratchet or skip exemption was added.

#923's 37 focused completion/guard cases and the Rust harness's 37 cases passed.
Ruff, clippy/fmt, Semgrep and committed-range Gitleaks exits are bound to the
final candidate in its task report and evidence manifest. The subsequent
documentation-only commit does not alter the tested source tree. Saved review
snapshots preserve both base and final head; author gates do not establish
independent acceptance, and imports/release remain held until its disposition.
