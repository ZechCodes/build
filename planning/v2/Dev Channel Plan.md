# Parallel dev stack — #781

Revised 2026-10-22 to Zech's lean decision. Planning only; no source or deployment changes.
Code baseline: `e5a9bdce8786fd7b44b498ecc2ca419fed184d92`.

## Decision and isolation

Use `https://dev.getbuild.ing/app/` and `wss://relay-dev.getbuild.ing`, with a
separate skriftapp deployment, Postgres, accounts, invite/passkey and secrets.
A dev bridge registers in the dev DB; the prod device list never sees it.
The dev relay authenticates only against the dev API, so prod device identities
and prod gateway tokens are unknown there; the reverse holds for prod.
`DevicesController` and `RelayState::open_session` keep their existing ownership
checks. No shared-record channel columns or production behavior changes.

A separate origin isolates localStorage, IndexedDB (`build-cache`, `build-ui`,
`build-push`), BroadcastChannels and service workers. Keep APIs same-origin,
cookies host-only with unique signing secrets, no cross-stack credentialed CORS,
and dev CSP restricted to the dev relay. Retain production-strength origin/CSRF
protections; verify actual cookie/header behavior rather than assuming SameSite
separates sibling hosts. Pairing keeps fresh Ed25519/X25519 keys, explicit
fingerprint approval and fragment-only links. A wrong-stack code is not found;
a wrong-stack token/device is refused by the existing auth/ownership path.

This boundary depends on correctly separated URLs, secrets and data; explicit
bridge overrides can deliberately select other endpoints/state. Do not copy prod
identities, DBs or browser credentials into dev. Separate accounts cost a second
sign-in and extra hosting, but avoid shared-auth/schema/deploy coupling. A dev
process still has the Unix user's file access: use disposable dev source clones.

Signed session-admission tickets, shared-record channel columns, relay-challenge
versioning and a production compatibility cutover are dropped. They are possible
future hardening, not prerequisites or work in these three tasks.

## Slice 1 — Dev deploy, config and CI (one task)

- Add `deploy/k8s/dev/` with its own kustomization/resource set: namespace
  `build-dev` for app, relay, Postgres/PVC, migration Job, Services and TLS ingresses.
  Use namespace-local DB URL/secret refs; relay `API_INTERNAL_URL` and legacy
  `RELAY_API_URL` both point to the dev app Service, never the prod app.
- Derive `skriftapp/app.channel-dev.yaml` from prod `app.yaml`, not relaxed
  `app.dev.yaml`. Set auth/public origin and passkey RP to `dev.getbuild.ing`,
  CSP `connect-src` relay to `wss://relay-dev.getbuild.ing`, and keep internal
  localhost auth bypass off. Keep `/app/` and `/app/static/` unchanged.
- `BuildController.index`, `_render_spa`, `static_asset`, `service_worker` in
  `skriftapp/buildapp/controllers.py` need no channel logic. Separate images
  preserve separate SPA output trees; keep document/worker/cache headers.
- Use existing `VITE_RELAY_URL` build arg in `skriftapp/Containerfile`; pass
  `wss://relay-dev.getbuild.ing` and `VITE_BUILD_VERSION=<source SHA>`. Add a
  config-selection build arg with the existing prod config as default, and
  `VITE_BUILD_CHANNEL` defaulting to prod for slice 3. Dev passes both explicitly.
- Add workflow-dispatch-only `.github/workflows/dev-deploy.yml`. Resolve the
  `dev` branch to one SHA, checkout/build that exact SHA, and record it. Publish
  `build-app-dev:<SHA>` and `build-relay-dev:<SHA>` only; never `latest`, prod
  repositories or release tags. Deploy pinned digests from the successful build.
- Use protected `development` environment and dev-only namespace-scoped kube
  credentials. Build/test jobs receive no deploy secrets. Apply only the dev
  namespaced resource set; keep Namespace/RBAC bootstrap outside its kustomization.
  Migrate dev DB before app rollout; keep relay Recreate and independent rollout
  concurrency. Failure/rollback affects dev only. Leave `ci.yml` prod behavior intact.
- **TDD first:** config tests for dev origin/CSP and unchanged prod defaults;
  manifest tests for namespace/secret/Service/PVC separation; workflow tests for
  dispatch-only, pinned dev SHA, forbidden prod/latest tags, protected deploy,
  failed-gate skip and dev-only apply. Disposable two-stack tests prove separate
  device lists, cross-stack code/token rejection and two-origin browser storage.
- **Gate tiers:** skriftapp ruff/pytest; SPA lint/test/build for build plumbing;
  bridge tests/clippy/fmt for relay image; web/transport interop for stack proof;
  shellcheck if scripts change; manifest/workflow checks, gitleaks and semgrep.

## Slice 2 — Bridge dev mode (one task)

- Add one switch, `BRIDGE_CHANNEL=dev`, resolved by `config.rs::resolve` and
  `main.rs::bridge_config`; absent retains every existing prod default. Unknown
  channel errors. Dev defaults API/web to `https://dev.getbuild.ing` and relay
  to `wss://relay-dev.getbuild.ing`. Existing individual `BRIDGE_*` overrides win.
- Consolidate defaults currently split across `config.rs` and `main.rs::serve`,
  `backup`, `default_tasks_dir`, `install_service` and `daemon_environment`.
  Dev root: `~/.local/state/build-dev/`, containing `identity.json`, `config.json`,
  `tasks/build.db` and docs/WAL/SHM, `projects/`, `worktrees/`, `mcp.sock`, `log/`.
  No prod SQLite, identity, log or MCP socket is reused by default.
- Propagate dev mode and resolved paths to service/MCP subprocesses; retain
  existing private-key stripping. Fresh pairing uses the dev identity and
  `pairing.rs::ensure_paired` prints the dev web approve link. Do not inherit
  provisioned production keys through `main.rs::provisioned_identity` in launcher.
- Parameterize service identity in `service/mod.rs`, `systemd.rs` and
  `launchd.rs`: Linux `build-bridge-dev.service`, macOS `ing.getbuild.bridge.dev`.
  Install/uninstall/status target only that mode's unit and dev logs.
- Disable dev update checks/install/scheduling at the bridge boundary, including
  `update/service.rs::UpdateService` request paths, scheduled staging/launch and
  `main.rs` update-helper dispatch. No dev helper/provenance/staging code touches
  `~/.build`; reject explicit release-replacement confirmation too. Prod unchanged.
- Build the pinned dev checkout with `nice -n 10 cargo build --locked --release`;
  use a separate binary path, e.g. `~/.local/lib/build-dev/<SHA>/build-bridge`.
  Optional `scripts/run-dev-bridge.sh` scopes the switch to that process and
  runs that binary; no production installer or global shell environment edits.
  No fixed bridge TCP port is needed: outbound relay, Unix MCP and ephemeral RTC
  sockets coexist. Dev containers may repeat internal ports in separate pods.
- **TDD first:** injected/temp-home config tests for both modes and override
  precedence; distinct identity/store/socket/log and backup paths; mocked unit
  install/uninstall targets; MCP mode propagation; dev checks/install/schedule/
  helper refusals with zero backend calls; unchanged prod update behavior.
  All runtime tests use temp homes and mocked service commands, never live daemons.
- **Gate tiers:** bridge tests/clippy/fmt, shellcheck for launcher, API fixtures
  if response contracts change. Gitleaks and applicable semgrep before review.

## Slice 3 — SPA dev label (one task)

- Export build-time channel from `spa/src/config.js` using
  `VITE_BUILD_CHANNEL`; unset/prod leaves current UI/title unchanged.
- Add a small persistent `Dev` badge beside `#global-controls` in `spa/index.html`,
  initialized by `spa/src/main.js` (including welcome/gate and narrow layouts),
  with styling in `spa/src/styles.css`. Set title to `Build Dev — review is the
  product`. Render from build config only, never connection state or device data.
- Pass the flag through the Containerfile/dev workflow plumbing from slice 1.
- **TDD first:** DOM tests for dev badge/title, absent prod badge/original title,
  persistence across routes and disconnected/empty device states; browser layout
  check on phone/desktop; build test proves dev flag is baked into dev output.
- **Gate tiers:** SPA lint/test/build, Containerfile/workflow checks if touched,
  gitleaks and applicable semgrep. No bridge or production auth changes.

## Zech provisions

- DNS records for `dev.getbuild.ing` and `relay-dev.getbuild.ing` to the ingress;
  TLS certificates/secrets for both hosts via the cluster issuer, HTTPS/WSS only.
- `build-dev` namespace, dev-only RBAC, Postgres 16/PVC and capacity. Secret
  `build-postgres`: fresh `POSTGRES_PASSWORD`; `build-app`: fresh `SECRET_KEY`,
  `DATABASE_URL` for dev Postgres, `INTERNAL_API_SECRET`; `build-relay`:
  `RELAY_INTERNAL_SECRET` matching only the dev app internal secret.
- Dev `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM_ADDRESS` (required by derived
  prod config); optional `WAITLIST_NOTIFY_ADDRESS`. Use a dev mail sink/account.
  Separate `VAPID_PRIVATE_KEY`, `VAPID_PUBLIC_KEY`, `VAPID_SUBJECT` for push;
  optional dev `CF_TURN_KEY_ID`/`CF_TURN_KEY_API_TOKEN` (absent means STUN-only).
- Protected `development` CI environment/reviewers, dev-only kube credential,
  GHCR push permissions for the two dev repositories and namespace-local
  `ghcr-pull` Secret. Create the `dev` branch and authorize manual deployments.
- Seed a new dev invite/account and register a new dev passkey. Authorize later
  local dev service installation/pairing separately; no prod release is needed.

Validation follows existing pairing/update/push security-checklist patterns:
write tests first, record passing evidence, never claim future controls verified.
This planning revision changes only this document and performs no live actions.
