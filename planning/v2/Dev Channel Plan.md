# Parallel dev channel

Planning deliverable for #781, 2026-10-22. Code inspected at
`e5a9bdce8786fd7b44b498ecc2ca419fed184d92`. This document proposes implementation;
none of the new controls below exists yet, and no deployment is authorized by
this plan.

## Recommendation and boundary

Serve `https://dev.getbuild.ing/app/` from a separate skriftapp deployment and
use `wss://relay-dev.getbuild.ing` for its separate relay. Keep `/app/`,
`/app/static/`, and the same application behavior on both hosts. Use separate
Postgres databases, accounts, session keys, internal secrets, admission signing
keys, TURN credentials and push keys. A dev account may use the same email, but
it is a new account, not the production account or a copied production record.
Start with manual, SHA-pinned deployment from a long-lived `dev` branch through
a protected `development` CI environment. Production continues to deploy only
from `main` through its existing workflow.

Make channel an immutable build/deployment authority, not a selector a browser
or a runtime environment variable can change. Enforce it at registration,
token validation, relay routing and, critically, bridge session admission before
unwrapping a session key. A dev bridge runs from its own binary and fresh state
under `~/.local/state/build-dev/`, with a separate unit and MCP socket.

The promised boundary is: a dev browser's credentials, pairings and session
requests cannot authorize access to a production bridge, and vice versa, even
if identifiers, public keys or tokens are copied across channels. A channel
string in JavaScript, a filtered picker, a different relay URL, or a late
`session.hello` check alone does not establish this boundary.

This does not sandbox a dev daemon running as the user's Unix account. It can
access that user's files and launch agents. Use disposable source clones and
dev projects, not the live Build checkout or production worktrees. Isolation
from arbitrary malicious native code requires an OS sandbox/container or another
Unix account, beyond this channel plan. Likewise, arbitrary native code with
stolen production private keys is outside the browser/channel threat model.

## Current code: what must change

Paths below are repository-relative; function names refer to inspected code.

| Area | Observed behavior and implication |
| --- | --- |
| Serving | `spa/vite.config.js` uses `/app/static/` and empties `skriftapp/buildapp/static/`. `BuildController.index`, `_render_spa`, `static_asset`, `service_worker` in `skriftapp/buildapp/controllers.py` serve `/app/`, substitute account identity, set document `no-store`, cache hashed assets immutably and serve a cache-free push worker at `/app/sw.js`. A second build in the same output directory would overwrite prod assets. Separate images avoid that. |
| Build/deploy | `skriftapp/Containerfile` builds transport, SPA and landing together, defaults relay to prod, and copies `app.yaml`. `.github/workflows/ci.yml` runs on main pushes/manual dispatch, checks tiers, publishes `build-app:{latest,SHA}` and `build-relay:{latest,SHA}`, migrates then rolls app, and recreates the single relay in namespace `8ly`. Image/deploy jobs explicitly require `refs/heads/main`. Releases and bridge self-update use `.github/workflows/release.yml`, separately from SPA deployment. |
| SPA config | `spa/src/config.js` exports `RELAY_URL`, defaulting to `ws://localhost:18090`. `validateDeployRelay` in Vite checks non-local relay for versioned builds, but does not check channel. `VITE_BUILD_VERSION=dev` means an unstamped build with the version watcher disabled; it is not a security channel. Stamp hosted dev builds with their real SHA. |
| Pairing | `DevicesController.register`, `lookup`, `approve`, `_pending_by_code`, `list_devices`, `revoke`, `rename`, `status`, `internal_device`, `gateway_token`, `internal_gateway_token` in `skriftapp/buildapp/devices_controller.py` know accounts and approval, not channels. `pairing_crypto.registration_challenge` binds device id, identity and transport keys and code hash. Raw codes travel in POST bodies; the approve link uses `#/pair/<code>`. |
| Credentials | `models.Device` stores Ed25519 identity and X25519 transport public keys. `models.EphemeralToken` stores a token hash, purpose, user and expiry. `ephemeral_tokens.mint` / `holder_of` and gateway lookup scope only by purpose/account. This is an opaque database-backed token, not a JWT with an existing audience claim. `desktop_auth.build_auth_guard` accepts session or desktop OAuth bearer, then checks alpha membership. |
| Relay | `RelayState::open_session` in `bridge/src/relay_server.rs` checks account ownership. `authenticate_device` verifies approval and Ed25519 signature. `bridge/src/bin/relay.rs::serve` captures path/device auth headers but currently no browser Origin allowlist. Its API lookups resolve device records and gateway token owners through a shared internal secret. Relay routing is not presently channel-scoped. |
| E2EE | `createRelayRendezvous` in `spa/src/core/rendezvous.js` seals session keys to `devices.js::pinnedDeviceTransportKey`, sourced from the API, never relay key discovery. `transport/js/src/index.js::createSessionInit` and `bridge/src/transport.rs::open_session_init` establish encryption, not an authenticated browser principal. Possession of a public transport key permits constructing a sealed init. |
| Bridge admission | `FrameIntake::open` in `bridge/src/carrier.rs` immediately unwraps the init, opens registry state and sends accept. `accept` then decrypts frames, handles signaling, and rejects application traffic over relay. Direct DataChannels are negotiated through that session. Checking channel only at RPC dispatch/hello would be too late, and terminal sessions need the same admission. |
| Identity | `identity.rs::StoredIdentity` has `approved_by`, but no channel. `pairing.rs::ensure_paired` trusts an approved local identity without a network check; `approved_by` controls which API may retire it, not channel membership. `main.rs::provisioned_identity` can bypass pairing using provisioned environment keys. Both paths require validation. |
| Local storage | `core/localCache.js` uses IndexedDB `build-cache`, `core/localUiStore.js` uses `build-ui`, and `pushKeys.js` uses `build-push`; selected device and other preferences use localStorage. Same-origin paths share these, BroadcastChannels and service worker authority. Different HTTPS hosts separate them without renaming every database. No prod storage import is allowed. |
| Compatibility | `ARCHITECTURE.md` documents `session.hello`, adapter/version gates and the direct-only application carrier. API version at this cut is 3.24.0. Version gating is UX/compatibility, not admission security. Do not repurpose durable-write epochs, client LAN hints or capability flags as channel credentials. |

## Serving, build and independent deployment

Add a production-strength dev app config (proposed `skriftapp/app.channel-dev.yaml`)
and `deploy/k8s/dev/` resources in a new namespace, e.g. `build-dev`. Do not use
`app.dev.yaml`: that local-development config deliberately relaxes CSP and is
not appropriate for a hosted dev app. Keep TLS, alpha membership, rate limits,
internal secret requirements and production server hardening. Set public base
URL, WebAuthn origin/RP ID, relay CSP target, DB URL and session secrets for the
dev host. Account creation still needs a dev invite; provision only test accounts.

Extend Containerfile inputs to select channel/config explicitly, with no
production fallback for a dev image. Add `VITE_BUILD_CHANNEL`, channel to
`version.json`, and build/start validation of the entire origin/relay/channel
tuple. Public values are not secrets. Dev build must fail if given the prod
relay, app origin, prod config or absent channel. Unknown channel is an error.
Production defaults may remain for existing prod builds during preparatory
slices, but the dev workflow must pass and verify all values explicitly.

Add `.github/workflows/dev-channel.yml`, initially `workflow_dispatch` only.
The workflow definition is reviewed on main; checkout accepts only a commit
reachable from `refs/heads/dev`, records its exact SHA, and gates the source
before building. Do not execute arbitrary branch-selected workflow code with
deployment secrets. Secrets are injected only in a separate protected deploy
job, not available to source build/test steps. Publish distinct repositories
`build-app-dev:<SHA>` and `build-relay-dev:<SHA>`, deploy by digest, and archive
the matching bridge-dev build artifact and manifest (channel, commit, hashes).
No production image tags, `latest`, release tags or release repository writes.

Use separate concurrency groups; never cancel an active rollout. Dev kube
credentials can mutate only `build-dev`, including its migration Job. Dev
database credentials cannot read prod or migrate its schema. Apply only the
dev resource set, never root `deploy/k8s/kustomization.yaml` or namespace `8ly`.
Follow current app migrate-before-roll and relay Recreate behavior in dev.
Rollback pins the previous dev digest; schema compatibility is required for
that rollback. Dev failures and DB resets must not change prod availability.
Keep prior dev hashed assets through a rollout or verify controlled reload
when an open dev tab references removed assets.

Same-origin `/dev/app/` is rejected: it shares storage, worker authority and
script privileges with prod, even with different cookie paths. A separate
origin routed to the prod backend also remains insufficient without server
credential and device scoping. The selected separate stack keeps API requests
same-origin (`spa/src/api.js`) and avoids a broad CORS exception.

## Isolation contract and enforcement

### Fixed authorities and fresh pairing

Introduce a small `ChannelPolicy` module on each side (proposed
`bridge/src/channel.rs`, `skriftapp/buildapp/channel.py`, `spa/src/core/channel.js`).
The bridge's build selects `prod`, `dev`, or a separately compiled local-test
profile. An ordinary environment override cannot convert a dev artifact into
prod. Bind its permitted HTTPS API/app origin, WSS relay, channel id and admission
verification key set into the artifact/reviewed policy. Reject cross-origin
redirects for authenticated bridge API calls. Local HTTP/WS endpoints belong
only to test artifacts, never the hosted dev profile.

Persist channel and canonical approving authority in new identities. Validate
them before `ensure_paired`'s approved fast path and before any provisioned
identity is admitted. Missing channel in old identities is legacy prod only;
dev never imports or retires these files. Reject mismatch without modifying
the identity or contacting another authority. Channel changes require a new
identity with new UUID, Ed25519 and X25519 keys, not `pair --retire` against
the other API. Refuse all environment-provisioned identities in hosted dev
initially; keep explicit test-only fixtures for compose.

Add immutable `Device.channel` (old rows default prod), channel to signed
registration challenges in both `pairing.rs` and `pairing_crypto.py`, and
channel in device responses. The API instance derives its own channel from
trusted config and refuses a different registration channel. Every lookup,
approve, list, status, rename/revoke, heartbeat, transport report and internal
device read is scoped to that channel, regardless of client-supplied device id.
Code lookup and approval cannot move a device between channels. Include channel
in fingerprint/approval presentation as context; preserve fingerprint checking,
POST code bodies, fragment clearing and explicit human approval.
Extend the device relay authentication challenge in `bridge/src/relay.rs` and
`relay_server.rs` to bind channel and relay audience as well as timestamp/path;
an unsigned channel header cannot select another authority. Version this
challenge explicitly, with a prod-only legacy adapter during rollout.

### Tokens, relay and origins

Gateway token minting uses a channel-specific purpose (`gateway:dev` versus
`gateway:prod`) and returns a channel-scoped internal auth result. Use separate
DBs/secrets as the primary separation and explicit audience/channel validation
as defense against accidental backend wiring. The relay's immutable deployment
channel must match both device and gateway record channels; unknown/missing
values on dev fail closed. `RelayState` keys device/client/session state with
channel and owner; `open_session` verifies all three before forwarding init.
Do not infer audience from a token prefix or a browser-provided channel.
Internal API failure or conflicting channel is an authentication failure.

Add exact serialized HTTPS Origin allowlists to `/ws/client` upgrade in
`bin/relay.rs::serve`, before accepting/authentication. Prod allows only
`https://getbuild.ing`, dev only `https://dev.getbuild.ing`. Reject missing,
`null`, sibling hosts, suffix matches and arbitrary ports for browser clients.
`/ws/device` retains signed native authentication, without requiring a browser
Origin. Desktop currently loads the hosted app; verify its actual Origin in a
test before rollout, and never add `null` or `*` as a desktop workaround.
An Origin check protects browsers; it is not authentication for native clients.

Keep prod and dev session cookies host-only, Secure and HttpOnly, with distinct
names and signing secrets; never `Domain=.getbuild.ing`. Sibling subdomains are
same-site, so SameSite alone does not stop dev-to-prod CSRF. Inspect the vendored
Skrift middleware settings at implementation and test actual emitted headers:
do not assume the YAML currently proves cookie/CSRF behavior. Enforce exact
browser Origin plus CSRF on authenticated mutation/token/admission endpoints,
including cookie-auth requests with absent/null Origin per explicit tested
policy. Maintain separately authenticated native device routes. Keep API
responses unreadable cross-origin: no credentialed CORS between channels.
No dev redirects into production auth. Separate signing secrets and DB OAuth
registrations also prevent prod desktop access/refresh tokens working on dev.

### Bridge admission: the decisive check

Add a short-lived server-signed admission ticket so the bridge does not trust a
self-asserted channel field or the relay. Proposed
`POST /api/session-admissions` uses `build_auth_guard`, exact-origin protection
and rate/body limits. It checks current channel, approved device ownership and
membership. Its Ed25519 signing key stays in the app Secret; prod and dev have
different keys. The matching public verification keys are pinned in bridge
artifacts. Key rotation is explicit and channel-local, never fetched from a
client-supplied URL or accepted by an arbitrary `kid`.

Ticket claims: format version, issuer/app origin, audience/channel, device UUID,
identity/transport public-key digest, session id, hash of the exact sealed init,
issued-at, expiry (initially 60 seconds), and random admission id. Sign a
domain-separated, fixed canonical encoding shared by Python/Rust golden
fixtures, not two ad hoc JSON serializers. The endpoint sees a digest and public
routing identifiers, never a plaintext session key. The device record supplies
the key digest, rather than accepting it as authority from the client.

In `createRelayRendezvous.mint`, construct the sealed init first, obtain a ticket
bound to it, and send `{session_init, admission}`. Carry the ticket as an outer
Build protocol extension, leaving the cryptographic binding's `SessionInit`
format intact. Relay routing checks channel but need not decrypt or mint tickets.
Both application and independently minted terminal sessions obtain tickets.

Pass admission metadata through all callers of `FrameIntake::open`. Verify its
signature, fixed issuer/audience, own device and key digest, bounded time window,
exact session id and init digest **before** `transport::open_session_init`,
registry insertion, accept or peer creation. Compare inner/outer ids too.
Maintain a bounded replay ledger: an exact repeat can rebind only the same
currently live opening under existing carrier rules; changed payloads and a
retired opening cannot reopen using that ticket. Retain consumed ids until
expiry plus bounded clock skew; expiry/restart cannot create an unlimited
replay window. A process restart loses sessions and must require fresh tickets.
Use a bounded durable consumed-ticket ledger in the channel's own SQLite, with
atomic consumption before session creation and expiry cleanup. Do not hold the
app-wide lock for that lookup. Exact in-process live rebinds use opening state;
after restart a consumed ticket is refused and the client mints another.

Audit `carrier/`, relay inbound handling and channel-borne init dispatch so a
future direct/LAN opening cannot skip the same verifier. Existing RTC signaling
and `app`/`term` DataChannels belong only to an admitted opening. Ticket expiry
limits new openings, not an established session's lifetime. Existing revocation
semantics for already-direct sessions are unchanged; instant revocation of a
held peer is a separate problem, not implied by a 60-second ticket.

Return bounded `channel_mismatch` / `admission_refused` errors. SPA connection
attempt handling makes mismatch terminal for that attempt, never falls back to
prod/local relay, and displays “This app connects only to dev bridges. Pair a
dev bridge at dev.getbuild.ing.” Wrong-channel pairing answers generic “No
pending dev device for that code” without disclosing prod records. Bridge CLI
errors name expected channel/origin without keys, tokens or raw code. Product
surfaces keep rendering cached state; the existing connection-status surface
owns the refusal. Add a persistent small Dev label/title and explicit dev text
in pairing/download instructions so users know which app they opened.

## Dev bridge on the same machine

Current defaults must not be inherited. `config.rs::resolve` defaults identity
to `~/.build/identity.json`, worktrees to `~/.build/worktrees`, and MCP socket to
`<worktrees>/build-bridge-mcp.sock`. `main.rs` independently resolves projects,
config and tasks; `default_tasks_dir()` is `~/.build/tasks`, whose `build.db`,
WAL and SHM hold the store. One config override does not relocate the others.

Implement a channel-owned root resolver, shared by serve, pair, backup, MCP,
services and update/provenance code. Initial proposed layout:

| Resource | Hosted dev value |
| --- | --- |
| State/config root (0700) | `~/.local/state/build-dev/` |
| Identity (0600) | `<root>/identity.json`, freshly generated |
| Settings | `<root>/config.json` |
| SQLite and canonical docs | `<root>/tasks/build.db`, associated WAL/SHM and docs |
| Cloned projects / worktrees | `<root>/projects/`, `<root>/worktrees/` |
| MCP socket / logs | `<root>/mcp.sock`, `<root>/log/` |
| Immutable binary copy | `~/.local/lib/build-dev/<SHA>/build-bridge-dev` |
| Linux user unit | `build-bridge-dev.service`, exact SHA binary in ExecStart |
| macOS follow-up equivalent | `ing.getbuild.bridge.dev` with dev-only paths |

Existing knobs are `BRIDGE_IDENTITY_FILE`, `BRIDGE_CONFIG`, `BRIDGE_TASKS_DIR`,
`BRIDGE_PROJECTS_DIR`, `BRIDGE_WORKTREES`, `BRIDGE_MCP_SOCKET`, `BRIDGE_API_URL`,
`BRIDGE_WEB_URL`, `BRIDGE_RELAY_URL`. Consolidate defaults in the resolver;
validate any override remains within the dev root using canonical ancestor
checks, rejecting aliases/symlinks/hard-linked database/identity paths into prod.
Use owner-only creation and refuse unsafe existing roots before writes. Keep
service environment complete and scoped to its own process; never export it in
the user's shell startup or rewrite the production service environment.
MCP subprocesses must use the dev binary/socket and retain channel scope while
dropping daemon private keys as `harness/mod.rs` already does.

No fixed inbound HTTP port is needed by the bridge: relay connection is outbound,
MCP is a Unix socket and RTC uses ephemeral UDP sockets. Two daemons may use
independent ephemeral ports normally. Do not invent a fixed port override;
test simultaneous allocation. Hosted dev app and relay use their own Services
(internal 8080 and 8799 may repeat in separate pods). A local test stack must
allocate different host-mapped ports from existing compose stacks.

Build with `nice -n 10 cargo build --locked --release` in a dedicated dev checkout
with the proposed dev build profile/policy inputs, recording source SHA. Copy
to its own immutable binary path after checks. Do not overwrite the binary
currently executing. Proposed launcher `scripts/run-dev-bridge.sh` validates
policy/root, starts only the explicit dev artifact and prints the dev approve
link. Begin with foreground launch; add the distinctly named user unit in the
same isolation slice only after unit-rendering tests pass. No automatic install,
restart, pairing or service enablement occurs during implementation tests.

Do not reuse `scripts/install.sh`, `install-service` or `uninstall-service`
unchanged. `service/systemd.rs` hardcodes `build-bridge.service`, service code
uses `ing.getbuild.bridge` on macOS, `main.rs::install_service` writes prod logs,
and `update/provenance.rs` hardcodes prod marker/unit locations. Initial dev
artifacts refuse production installer and self-update operations, including
`replace_development_build: true`, scheduled updates and helper launch. A source
build currently classed as “development” by provenance can be replaced by a
release with confirmation; that is **not** the new dev channel policy. Rebuild
dev from dev SHA manually. Leave public prod downloads/installers unchanged;
dev `/app/downloads` must provide dev setup instructions/artifact only, never
offer the production installer or release replacement action.

## Accounts and topology trade-offs

| Option | Benefits | Costs / risks | Decision |
| --- | --- | --- | --- |
| Same app/accounts and relay with channel tag | One sign-in, fewer services | Every device/token/pairing/push/internal query and relay map must partition correctly; dev backend/schema deploys still risk real accounts; branch SPA needs separate origin/backend routing anyway | Defer; not the recommended first implementation |
| Same account authority, separate dev relay | Less relay risk, shared login possible | Needs scoped identity/token exchange and prod auth dependency; shared DB/migrations retain blast radius | More machinery than needed for one developer |
| Separate app/accounts/DB/relay | Dev changes cannot migrate prod DB or restart prod relay; hard separation of credentials and pairing; existing same-origin API paths retained | New dev login/invite/passkey, extra hosting and secrets, independent backups and migrations | Recommended |

Do not clone the production DB, bridge identity, notification keys or OAuth
refresh tokens into dev. Seed only test accounts/content. Disable outbound mail
and push initially, or use a dev sink; enabling them later requires distinct
VAPID keys, subscription storage and dev URL validation. Use a separate TURN
credential authority or dev-only provider key/quota; do not share long-lived
production TURN minting secrets. An unavailable dev relay/API must cause a dev
failure, never retry through prod. WebRTC may use shared public STUN endpoints;
STUN is not application admission or account identity.

## Security checklist and required proof

Follow the threat/control/status/evidence pattern in `Device Pairing Security
Checklist.md`, `Bridge Update Security Checklist.md`, `Chat Isolation Security
Checklist.md`, `Desktop Security Checklist.md` and `Push Content Security
Checklist.md`. Create a dedicated Dev Channel Security Checklist during
implementation. The following are requirements, **not verified controls**.
All must be checked with named passing evidence before scoring 100/100 or
enabling the hosted app.

| Control | Required evidence |
| --- | --- |
| Cross-channel registration and pairing are impossible | Python/Rust signed-challenge fixtures; copied prod code/id/key refused on dev and reverse; dev never retires an old prod identity |
| Tokens are not reusable across channels | Prod gateway/session cookie/OAuth bearer rejected on dev and reverse, even same email/UUID; purpose, issuer, secret and API-miswiring cases |
| Both relay authentication and routing enforce channel | Cross-product of app/device channel, same-owner and different-owner; no forwarded init on failure; injected/missing channel and existing socket revalidation cases |
| Bridge refuses before crypto/session/RTC admission | Valid opposite-channel ticket, forged/absent/expired ticket, wrong issuer/key/id/hash, altered sealed init, inner/outer id mismatch, replay/restart and terminal/direct-carrier tests; assert no registry entry, accept, peer or RPC side effect |
| A dev page cannot use ambient prod cookies | Real browser two-origin tests of token minting, approval, revoke and admission endpoints; exact Origin + CSRF, denied preflight/read, host-only cookie headers and no parent-domain cookie trust |
| CSP and WebSocket origin controls survive deployment | Header tests on both actual host routes: prod relay excluded from dev connect-src and reverse; browser WS wrong/null/missing Origin refused; no CDN, JS eval or local-development CSP relaxation |
| State and process paths never alias prod | Temporary-home tests for every entry point, root/symlink/hardlink refusal, distinct unit/binary/socket/log/db/backup paths; two disposable daemons with independent schema migration and MCP dispatch |
| Update/install cannot cross channel | Dev install/update/helper/scheduled replacement refusals even when confirmation or prod provenance marker is supplied; dev download UI never serves prod one-liner |
| Cache and notification state remain separate | Same-browser two-host test proves no selected-device, IndexedDB, BroadcastChannel, service worker, draft or push-key reuse; sign-out/reset on one host leaves other host intact |
| Dev deploy cannot mutate prod | CI workflow policy tests, namespace-limited RBAC denial, dev DB grants, distinct image repositories/digests, red-gate deploy skip and rollback rehearsal; no production secret in dev environment |

Logs must redact pairing codes, gateway/admission tokens, keys, cookies and TURN
credentials. Bound ticket/body sizes, parsing, replay-ledger growth, retries and
error categories; admission API must reuse account/device rate budgets. CSP is
defense in depth: WebRTC is authorized by bridge admission, not by assuming
`connect-src` blocks every peer candidate. Same-origin dev XSS can operate dev
bridges; prevent it from obtaining prod authority. Sharing a DNS parent does
not make arbitrary sibling origins trusted.

## Ordered implementation slices and gates

Every slice is independently reviewable and mergeable with hosted dev disabled.
Write the named negative tests first, observe failure, then implement. An
additive preparation slice must not enable an unenforced channel. Tiers below
are the AGENTS.md gates, all heavy commands under `nice -n 10`.

1. **Channel policy, identity and path foundation (bridge + skriftapp).** Add
   typed policies/config validation, identity metadata and signed registration
   version, additive DB channel column defaulting old records to prod. Centralize
   dev paths and reject unsafe identities/overrides. Cover approved fast path,
   provisioned identity, redirects and fresh-key generation. Existing prod stays
   compatible; dev requires new format. Gates: bridge tests/clippy/fmt;
   skriftapp ruff/pytest. No dev network endpoint enabled.
2. **Server and relay channel admission (skriftapp + bridge relay + shell/CI
   tests where changed).** Scope all device/pairing/heartbeat/report/internal
   routes, channel gateway purposes and relay maps. Add exact Origin/CSRF and
   cookie-header tests, including desktop regression. Old prod records/tokens
   remain prod only through an explicit compatibility adapter; dev never has
   this adapter. Deploy prod relay origin protection before exposing dev.
   Gates: skriftapp, bridge, web QA/interop as affected; shellcheck for scripts.
3. **Signed session-admission protocol (skriftapp + bridge + SPA; transport
   compatibility).** Add endpoint/tickets/replay store and verifier at
   `FrameIntake::open`, then integrate SPA app and terminal minting. Golden
   signatures, size/time/replay tests and opposite-channel direct-carrier tests
   are the acceptance gate. Add hello channel for diagnosis and update fixtures
   and API versions when the contract changes; do not guess a future version
   number in this plan. Gates: bridge, SPA lint/test/build, skriftapp,
   transport JS/Python/Rust interop and API fixtures. No security fallback after
   the enforcement cutover.
4. **Parallel bridge lifecycle (bridge + shell + SPA download/update UI).**
   Add dev artifact/launcher/unit rendering and full root resolver coverage;
   dev forbids prod install/update. Exercise two disposable daemons and MCP
   sockets using temporary homes and test services, never the user's daemon.
   Gates: bridge, shellcheck, SPA; macOS service tests if included. Linux
   foreground launch is the initial supported path if macOS packaging is
   deferred explicitly. No live install required to merge.
5. **Hosted SPA and dev infrastructure (SPA + skriftapp + deployment config).**
   Channel/version stamps, fail-closed build tuple, persistent Dev label,
   mismatch handling, dev production-strength config, separate namespace/DB/
   secrets/relay ingress and disabled mail/push. Test serving cache/header
   contracts and two-origin storage isolation. Gates: SPA, skriftapp,
   Containerfile context checks, deployment manifest/policy checks. No apply
   against live infrastructure during implementation.
6. **Dev-only CI and release cutover (workflow + shell + integration gates).**
   Add manually gated dev workflow, SHA/digest artifacts, separate credentials,
   branch validation and changed-tier tests in `.github/changed-tiers.sh` /
   `.github/test-changed-tiers.sh` if classification changes. Run full two-stack
   browser/relay/bridge matrix, RBAC denial and rollback rehearsal in disposable
   infrastructure. Gates: affected bridge/SPA/skriftapp/web/transport/shell
   plus gitleaks and semgrep. Final security checklist must reach 100/100 on
   actual evidence. Only then request deployment/release authorization.

Rollout needs an explicit compatibility boundary. Additive code may merge
before tickets are mandatory, but do not advertise full isolation then. First
ship the compatible production API/SPA/relay plumbing and origin restrictions;
release bridge admission enforcement with an appropriate minimum client/API
contract, update the production SPA, then require tickets for all new openings
and reject unsupported legacy clients. A minimum SPA/API version cannot upgrade
an installed daemon. At the final cutover remove the prod-only legacy relay
challenge adapter: require the new audience/channel-bound signed challenge and
admission-capable enrollment protocol for `/ws/device`, reject old daemons
before registration, and revalidate/close any relay device connections admitted
under the old protocol. Record protocol support from that authenticated exchange,
not an unsigned version string. Old prod bridges must upgrade (or remain
unreachable); do not claim they perform pre-unwrap verification. This is a
deliberate production compatibility break requiring release authorization.
Do not enable hosted dev until the
protected production boundary is deployed and tested. Existing pre-enforcement
direct sessions cannot acquire new channel guarantees retroactively; either
let them end before declaring cutover complete or use a separately authorized
restart/reconnect rollout. No automatic service restart is part of this plan.
Keep production releases and this dev workflow separate; bridge release
authorization belongs to Zech. Rollbacks must not reopen legacy admission
while dev remains enabled: disable dev access first or roll to an enforced
known-good version.

## Decisions and provisioning needed from Zech

The recommendation is concrete enough to implement; these are deployment
decisions, not a reason to leave the plan unfinished:

- Approve `dev.getbuild.ing` / `relay-dev.getbuild.ing` (or provide alternatives),
  DNS records, TLS ingress and the extra app/relay/Postgres capacity.
- Approve a separate dev login/invite and passkey registration rather than
  production account sharing. Designate who may enter the dev environment.
- Create the protected `development` CI environment, reviewer/branch rules,
  dev-only registry permissions and namespace-limited deploy credential.
  Approve manual dev deployments from the `dev` branch initially.
- Provision unique session/internal/admission secrets and dev DB grants; decide
  TURN quota/provider credential and whether mail/push remain disabled. No
  production secrets or user data should be copied as a shortcut.
- Authorize the later production boundary rollout/minimum-version transition
  and bridge release separately; choose the reconnect window. Authorize local
  dev service installation/pairing only after its isolated artifact is ready.

This task performs none of those live actions. Planning validation consists of
source inspection, documentation review, `git diff --check main..HEAD`, and the
required history secret scan; future tests above are acceptance criteria, not
claimed passing results.
