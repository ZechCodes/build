# Hosted bridge HTTP dependency and interface review — #910

Reviewed 2026-10-29 against frozen base
`fd2341443e0ffb88cdad7e4c9bb73ff53eb16269`. This records the hosted dependency
selection and actual HTTP authority boundary; deployment remains outside this task.

## Dependencies

* **PyJWT 2.15.1**, MIT, new exact direct dependency. PyPI uploaded the wheel on
  2026-09-28 (lock records its SHA-256), satisfying the existing seven-day cooldown.
  No new dependency is introduced by the wheel; EdDSA uses the already locked
  `cryptography` backend. We do not install a JOSE network/JWK client or implement
  signature framing. [Upstream changelog](https://pyjwt.readthedocs.io/en/stable/changelog.html)
  and [maintainer advisories](https://github.com/jpadilla/pyjwt/security/advisories)
  were inspected. The September recursion/OKP/JWK/options regressions are addressed
  by 2.15.x; 2.15.1 additionally corrects padding compatibility. This adapter uses
  a stored raw Ed25519 public key, never JWK URLs or embedded keys, restricts the
  whole protected header to `alg=EdDSA` and the expected type, supplies a new options
  dictionary, bounds input before decoding, and independently validates integer
  times against one request timestamp. Actual Rust `jsonwebtoken` signatures are
  checked by the hosted validator in `test_bridge_http_proofs.py`.
* **idna 3.18**, BSD-3-Clause, now an exact direct dependency because the adapter
  imports it. The version and artifacts already existed in the reviewed lock;
  promoting it adds no packages or artifact changes. The standard library's IDNA
  2003 processing incorrectly maps `faß.de` to `fass.de`; UTS46 with STD3 processing
  agrees with the Rust URL origin contract (`xn--fa-hia.de`). IP encodings that
  become numeric only after Unicode mapping are rejected. [Maintainer usage and
  license metadata](https://pypi.org/project/idna/3.18/) and
  [maintainer advisories](https://github.com/kjd/idna/security/advisories) were
  inspected. No DNS lookup is involved.

The lock changes only add PyJWT artifacts and the two direct requirements.
Existing provider and TLS dependencies remain locked. This review does not claim
that a package can have no undiscovered vulnerability.

## Hosted interface

All absolute audiences derive from the configured approving origin, never a request
Host or a proof's key/URL. All three compact-JWS types use paired-device Ed25519
keys, `iss=sub=device UUID`, a scalar exact audience, integer `iat/exp`, positive
lifetime at most 30 seconds, at most five seconds skew and a 16–128 character jti.
Registration, discovery and introspection types are disjoint.

* `POST /api/devices/{id}/http-endpoint/challenge`, JSON `{}`, returns
  `{challenge_id, nonce, expires_at}` (Unix seconds, 60 seconds).
* `POST /api/devices/{id}/http-endpoint`, `{proof}`, accepts signed registration
  claims `challenge_id, nonce, origin, endpoint_epoch, previous_endpoint_epoch,
  enabled`. Its type is `build-http-registration+jwt`, audience is this exact
  endpoint. The predecessor epoch must match; state changes require a new epoch.
* `POST /api/bridge-discovery/challenges`, guarded `{origin}`, returns
  `{challenge_id, nonce, origin, expires_at, issuer, account_id, resume_url}`.
  Resume route is `/app/?bridge_discovery={id}`. Guarded GET of the challenge path
  returns the same data to the initiating client while pending. Maximum eight live
  challenges per account; account/session IDs cannot be supplied as authority.
* `POST /api/bridge-discovery/complete`, guarded `{challenge_id, proof}`, validates
  type `build-http-discovery+jwt`, audience this exact completion URL, and signed
  `issuer, origin, endpoint_epoch, challenge_id, nonce` against current registration
  and the authenticated account/client. It returns the initial credential response.
* `POST /api/devices/{id}/http-credentials`, guarded `{family_id, endpoint_epoch}`,
  returns a new access token in the same live family after all ownership checks.
  DELETE of `/api/devices/{id}/http-credentials/{family_id}` revokes that binding.
* `POST /api/bridge-http/introspect`, `{token, client_assertion}`, requires type
  `build-http-introspection+jwt`, audience this exact URL, and signed `token_hash`
  (SHA-256 hex). Return `{active:false}` for an unknown, wrong-purpose/target,
  expired/revoked token. Active response includes `issuer, account_id, device_id,
  family_id, origin, endpoint_epoch, authorization_epoch, scope, exp, token_id`.
  A failed assertion is 401. The bridge's verifier applies its independent
  request-start-plus-30-second hard deadline; hosted service does not cache authority.

Credential responses contain `issuer, account_id, device_id, origin,
endpoint_epoch, family_id, access_token, expires_at, scope, name, fingerprint`.
Times are Unix seconds. Scope is the OAuth space-delimited string
`bridge:read bridge:write`; event subscriptions require read authority. Tokens use
32 random bytes and a separate `bhttp_` prefix, are stored only by SHA-256 hash,
and expire in five minutes at most. Every credential response is no-store.

## Issuing client and logout boundary

Browser families bind to an unpredictable session value inside the signed hosted
cookie, scoped to its authenticated account. Successful framework CSRF-protected
logout revokes that binding's families and pending challenges. A durable logout
tombstone prevents captured pre-logout cookie bindings from reminting HTTP authority.

Skrift's existing access JWT omits its refresh-family identity. Desktop families
therefore bind conservatively to the exact **verified access jti and immutable exp**
plus account/client identity. Family and child token expiry never exceeds that
parent exp. Introspection/renewal also check Skrift's access-jti revocation table.
A different OAuth access jti requires fresh discovery. There is no claim of stable
installation identity or refresh-spanning family continuity. Authenticated
`POST /api/bridge-http/logout` with JSON `{}` revokes the exact current parent
binding; individual DELETE supports explicit family cleanup before token rotation.
An offline logout cannot deliver a hosted revocation event: tokens expire within
five minutes and the bridge's thirty-second grant deadline remains independently
capped by token expiry. The client must clear local credentials and handles.

Device revocation increments its authorization epoch and revokes registration and
families in one transaction. All grants recheck current approval, owner, active
account, alpha membership, origin, endpoint epoch, authorization epoch and family
state. Unique `(device,jti)` records and conditional challenge updates prevent
replay across workers. Per-device live replay records cap at 1,024; exhaustion
refuses work instead of evicting live guards. Expired replay/token/challenge rows
are pruned during their respective transactions.

App HTML receives one enforced CSP assembled inside the pinned framework security
middleware. The fixed policy remains, with at most sixteen current owned endpoint
origins and the initiating client's one live pending candidate. Other accounts,
expired/consumed challenges and disabled/revoked registrations cannot extend the
next document policy. Existing loaded documents retain their reachability policy;
server authorization, expiry and client handle closure enforce revocation. No
hosted arbitrary URL fetch, DNS probe, redirect-following credential client, or
global `https:` connect permission is added.

## Evidence and limitations

Meaningful red runs observed the missing service/controller/migration, missing
logout tombstone, uncapped desktop parent token expiry, wrong IDNA processing,
suspended-account admission and Unicode IP alias acceptance. Targeted tests then
passed for those behaviors, independent-connection replay races, forwarder and
cross-account rejection, wrong-device introspection, replay-budget refusal,
endpoint changes, actual framework logout and CSP cookie/nonce ordering. The
migration is executed against legacy SQLite rows and reversed; metadata/index/FK
parity is checked. A real PostgreSQL deployment and cross-slice live HTTPS/browser
qualification remain coordinator integration work; these tests do not certify a
public rollout or completion of the entire dual-transport security matrix.

Final hosted evidence: full `nice -n 10 uv run --frozen python -m pytest buildapp
-q --disable-warnings` exited 0 with 1,090 passed / 10 skipped. The skips were
nine pre-existing disposable PostgreSQL proofs (one migration/bootstrap test,
four column-privilege cases and four schema-create cases) and one generated
landing asset check. No bridge HTTP test was skipped. After the final parent-expiry,
identity-fingerprint and unpadded-key changes, the six bridge HTTP test modules
passed 62 tests with exit 0; Ruff's complete hosted gate exited 0.

The first Semgrep run reported three unverified-decode findings. Negative fixture
construction now begins from verified fixture claims. The sole production
unverified decode has one exact-rule suppression with a local rationale: its
untrusted issuer selects a database-pinned key and cannot authorize, consume a
challenge or mutate state; the separate verifier enforces the signature and
complete profile before authority is considered. Wrong-key/type/audience tests
exercise that boundary. The final eleven-file Semgrep run exited 0 with zero
findings; no global rule was disabled. The policy slice separately scanned its
four Python files with zero findings.

Registration persists the SHA-256 identity fingerprint as well as owner/epoch.
Every credential verification compares it to the current paired key. Changing the
paired identity invalidates the old registration and requires a fresh endpoint
epoch to register again; previous grants remain revoked. Paired public keys accept
both existing standard-base64 encodings, with or without trailing padding, under
strict alphabet validation. A Rust-signed proof verifies against the real bridge's
unpadded persisted key representation.
