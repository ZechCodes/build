# Build v2 — Handoff

Status: **production-shaped and ready to cut over to getbuild.ing.** The stack
runs locally under podman compose in exactly the production topology, all test
suites and lints are green, and the k8s manifests + runbook are in
`deploy/k8s/`. The only remaining work is operational (secrets bootstrap, image
push, the zechcodes teardown the user runs by hand) — see
[`deploy/k8s/CUTOVER.md`](deploy/k8s/CUTOVER.md).

## Topology (final)

```
                    getbuild.ing                relay.getbuild.ing
┌─────────┐  HTTPS ┌────────────┐  /internal/*  ┌────────────┐  wss ┌─────────┐
│ browser │◄──────►│ skriftapp  │◄──────────────│ Rust relay │◄────►│ bridge  │
│  (SPA)  │        │ api + SPA  │ X-Internal-   │ ciphertext │      │ (user's │
└────┬────┘        └─────┬──────┘    Secret     │    only    │      │  box)   │
     │                   ▼                      └────────────┘      └─────────┘
     │             Postgres 16                        ▲
     └────────── wss /ws/client ──────────────────────┘
```

- **skriftapp/** — the Python app server on the Skrift framework: passkey auth
  (dummy auth is dev-only), device registry + approval/pairing, 5-min gateway
  tokens, content-free web push, and it serves the built SPA at `/app/`.
- **bridge/src/bin/relay.rs** (+ `relay_server.rs`) — the single Rust broker.
  `/ws/device` (Ed25519 challenge auth against the registry) and `/ws/client`
  (gateway-token auth) on one process. Validates everything against skriftapp
  over `/internal/*` with `X-Internal-Secret`. Forwards opaque envelopes only —
  it never holds a session key.
- **bridge/** — the Rust device daemon (user machines, never deployed):
  worktree-per-task, full-PTY harnesses, single `done` MCP tool, git-diff
  watcher, durable task store with boot recovery, E2EE transport, wss relay
  client, device pairing.
- **spa/** — the web client: Vite vanilla-ES-module app (no framework), every
  dependency self-hosted (libsodium, `@build/secure-transport`, ghostty-web
  with inlined wasm, Inter fonts). Zero CDN. Builds into
  `skriftapp/buildapp/static/`.
- **Postgres 16** — users, devices, sessions, push subscriptions.

**Retired:** `gateway/` (Node shim) and Redis are out of the topology — browsers
talk straight to the relay. `frontend/` (dead React scaffold) is deleted.
`web/` remains as the Node E2EE test/QA harness only.

## The browser↔relay contract

1. `POST /api/gateway-token` (Skrift-session authed) → `{token}`, 5-min TTL.
2. `GET /api/devices` → `[{device_id, approved, status, transport_public_key_b64, …}]`.
3. WS `/ws/client`; first frame `{"type":"authenticate","token":…}`; relay
   validates via the api and replies `{"type":"authenticated"}` or closes.
4. Client seals a fresh session key to the **api-pinned** device transport key
   and sends `session_init` with `route_to: "device:<id>"`; the device answers
   `session_accept` (protocol unchanged).
5. Relay pushes `device_online`/`device_offline` to that user's clients.
6. App frames are opaque encrypted envelopes; the relay never decrypts.
7. Relay→api internal calls carry `X-Internal-Secret: $INTERNAL_API_SECRET`.

## Run it locally

```bash
podman compose -f deploy/compose.real.yml up -d --build
podman compose -f deploy/compose.real.yml --profile qa run --rm qa   # pair + e2e + qa checks
open http://localhost:8090/app/        # dummy login (any email), dev-only
podman compose -f deploy/compose.real.yml down                       # resets all state
```

Services: `app` (skriftapp, :8090), `relay` (:18090), `bridge`
(`BRIDGE_QA_AGENT=1` on a sample repo), one-shot `qa`. Pairing is the real
device-initiated flow with a deterministic code (`COMPOSE-PAIR`); approve it
via the qa one-shot or in the SPA under Settings → Devices. Details:
[`deploy/README.md`](deploy/README.md).

Dev loops without containers:

```bash
cd bridge && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd skriftapp && uv run pytest
cd spa && npm test && npm run build     # emits skriftapp/buildapp/static/
```

`spa/` expects a sibling checkout of `build-secure-transport` next to this repo
(`file:../../build-secure-transport/js`), same as `web/` and CI.

## Deploy

Everything is in [`deploy/k8s/`](deploy/k8s/) (kustomize, namespace `8ly`,
hosts `getbuild.ing` + `relay.getbuild.ing`, images `ghcr.io/8ly-dev/build-app`
and `ghcr.io/8ly-dev/build-relay`, TLS via cert-manager
`letsencrypt-production`, Postgres on `do-block-storage-retain`). The
step-by-step runbook — secrets bootstrap, first image push, apply, pre-flip
verification, the **user-run** zechcodes teardown, rollback — is
[`deploy/k8s/CUTOVER.md`](deploy/k8s/CUTOVER.md). Images are rebuilt/pushed and rolled
out by the deploy stages of `.github/workflows/ci.yml` on pushes to `main` —
gated behind that workflow's checks, so a red commit builds no image.

## What this branch changed (highlights)

- **Persistence** — the bridge's task store is durable (one JSON file per task,
  atomic writes), with an `Interrupted(phase)` state and boot recovery/reattach.
- **TLS** — the bridge speaks `wss://` (rustls + webpki roots); both ingresses
  are pinned TLS-only.
- **Auth** — Skrift passkeys in production config; dummy auth is dev-only.
  Strict CSP; no inline route auth (shared session/guard helpers everywhere).
- **Single broker** — the Rust relay grew `/ws/client` (token auth, per-user
  device scoping, online/offline fanout, `/health`, bounded queues, graceful
  shutdown, replay guards); `gateway/` + Redis retired.
- **SPA modularization** — `build.html` (1,272 lines, esm.sh CDN imports, a
  stray NUL byte) became the tested Vite app in `spa/`; `frontend/` deleted;
  all crypto/deps self-hosted.
- **Web push** — content-free notifications: device-signed notify with a replay
  guard, VAPID keys generated by the secrets bootstrap, cache-free service
  worker, per-subscription failure isolation.
- **Secrets hygiene** — no secrets in git; `bootstrap-secrets.sh` is idempotent
  and never regenerates existing values; internal endpoints guarded by
  `X-Internal-Secret`.

## Known gaps / deferred

1. **Real LLM harness `done`-forward** — the per-task control socket so a real
   CLI agent's MCP `done` reaches the orchestrator; the compose/QA stack uses
   the deterministic scripted agent (`BRIDGE_QA_AGENT=1`).
2. **External crypto audit** — the Rust transport is a port of the audited
   protocol and is interop-verified (Rust↔Python↔JS), but is not itself audited.
3. **Stale QA harnesses** — `web/qa-reconnect.mjs`, `web/term-verify.mjs`,
   `web/term-browser.mjs` still speak the pre-auth protocol (see
   `deploy/README.md`).
4. **Bridge re-pairing after cutover** — the v2 registry starts empty; every
   existing bridge re-pairs against `https://getbuild.ing`.
