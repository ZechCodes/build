# Account-bound HTTP authorization implementation — #910

Base: `fd2341443e0ffb88cdad7e4c9bb73ff53eb16269` (reviewed contracts), with source
base `12b8f869bd94e259666df69f1a8c0cde5c695982`. The controlling architecture is
[the frozen review](../Bridge%20Dual%20Transport%20Architecture%20Review.md), plus
the #916 desktop parent-credential qualification. This document describes the
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

- `discovery_proof(challenge_id, nonce, now)` signs configured origin/epoch/issuer,
  device identity, and the fixed hosted completion audience.
- `registration_proof(challenge_id, nonce, previous_epoch, enabled, now)` signs
  the exact predecessor and complete endpoint registration state.
- `introspection_assertion(token_hash, now)` signs the SHA-256 bearer digest and
  fixed introspection audience. It cannot sign caller-supplied claims/audiences.

All three assertions use EdDSA compact JWS, distinct explicit `typ`, random `jti`,
30-second lifetime and the existing paired Ed25519 seed. The hosted verifier
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
| `POST /api/devices/{device}/http-endpoint/challenge` with `{}` | `{challenge_id, nonce, expires_at}`; no account credential; paired-device registration proof required on the next request |
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
- `aws-lc-rs =1.17.0`: same provider version as the frozen production Cargo lock.
  Its library imports the existing seed/public pair and serializes PKCS#8 for JOSE;
  no handwritten ASN.1, custom signature scheme, new RNG or extra provider.
- Hosted `PyJWT =2.15.1` uses existing cryptography Ed25519 and explicit validators.
  [Release/provenance](https://pypi.org/project/PyJWT/2.15.1/). `idna =3.18` pins the
  already-locked UTS46 implementation for origin equality with Rust URL parsing.
- `reqwest =0.13.4` in the harness matches the frozen production lock and disables
  redirects for credentials/assertions. No insecure certificate mode is added.

Cross-language fixtures are `fixtures/bridge-http/device-proofs.json`, emitted
by the harness `proof_vectors` example using a public fixed test seed. Python
runs all three through the real PyJWT validators, including purpose/key/claim
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
