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
cd skriftapp && uv run --frozen ruff check buildapp && uv run --frozen pytest buildapp
cd spa && npm run lint && npm test && npm run build     # emits skriftapp/buildapp/static/
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

## 2026-09-02 — Agent surfaces

Branch `sc-trapped-dewar-4eba`. Implements
[`planning/v2/Agent Surfaces Spec.md`](planning/v2/Agent%20Surfaces%20Spec.md)
(locked 2026-09-01). An agent's conversation now shows what the harness runs
*beside* it: multi-agent workflows, subagents, background shells, and the
agent's own checklist.

### Shipped

- **`bridge/src/harness/surfaces.rs`** — a `SurfaceLedger` that reads the
  Claude Code stream-json task events (`task_started` / `task_progress` /
  `task_updated` / `task_notification`) and routes them by `task_type` into
  four snapshot kinds: `workflows`, `subagents`, `shells`, `checklist`. A
  workflow's `workflow_progress` array is a full snapshot each time and
  replaces rather than merges. A kind with no content is omitted from the
  wire, never sent empty. Unknown state tokens claim nothing.
- **`bridge/src/harness/shell_tail.rs`** — reads the last 20 lines of a
  background shell's output file by seeking back from the end, and scrapes the
  `[exited with code N]` marker. Capped at 20 lines, 2048 characters per line,
  256 KB read. The tail poller runs once a second only while the shell set is
  non-empty.
- **Snapshot plumbing** — `AgentSession` gained a surfaces accessor and a
  content-free revision watch, parallel to the existing terminal and activity
  accessors. The per-session pump in `app.rs` selects on that revision and
  marks the owning entity changed, so a progress line that mints no thread row
  still invalidates. `agent_digest` carries `surfaces` on the four detail
  verbs (`branch.get`, `issue.get`, `run.get`, `plan.get`) and never on board
  list digests.
- **Subagent transcript rows** — thread events gained an optional
  `parent_sequence`. The pump maps `parent_tool_use_id` to the spawning
  call's sequence through the pairing map it already keeps, so a subagent's
  own messages fold under the call that spawned them instead of being dropped.
- **`spa/src/core/agentSurfacesModel.js`** — the pure model: one keying
  function, one state table, four named row kinds, and the canned row actions.
  No raw harness phases reach a row.
- **`spa/src/core/agentSurfacesRender.js`** — one row renderer shared by the
  workflow and subagent viewers, plus the shell and checklist viewers and the
  pills. Every model-supplied string goes through `esc`.
- **`spa/src/core/agentSurfaces.js`** — mounts the pills between the pinned
  status line and the composer in `agentRail.js`, with the viewer above them.
  Pills are pressed-state toggles, one open at a time; the open choice is
  remembered per agent in local storage. The viewer is painted with the keyed
  reconciler (`patchList`), so a growing workflow keeps row identity and
  scroll position, and a poll never shuts an open menu.
- **Recorded wire** — `bridge/tests/fixtures/agent_surfaces.json` is one
  snapshot both languages read: the bridge pins what it builds against it and
  the SPA tests parse the same file, so the two sides cannot drift.
- **Bounding (2026-09-02 security pass)** — every agent-supplied free-text
  field now rides the wire clipped to one 240-character line: workflow name
  and description, phase title, agent label, model, tool summary, result
  preview, error, and the subagent label. Only the checklist's text was
  clipped before. Unbounded, a single sprawling result preview could push an
  entity detail payload past the relay's 8 MB frame cap and leave the entity
  unopenable.

### Unverified

Everything below is *not* covered by an automated test on this branch.

1. **The browser-visible UI.** The SPA tests are jsdom: they assert markup,
   keyed patching, escaping, storage, and event wiring. Nobody has looked at
   the pills or the four viewers in a real browser. Layout, the narrow-width
   stack of the workflow viewer, the viewer's height cap and scroll, the live
   dot, and the CSS added in `spa/src/styles/shell.css` are all unconfirmed
   visually.
2. **A real Claude Code session.** Every bridge test is driven by recorded
   fixtures in `bridge/tests/fixtures/claude-stream/` and a fake child that
   replays them. No live `claude` CLI has been run against this code, so a
   field-shape drift in a CLI newer than 2.1.257 would not be caught. None of
   the workflow fields are documented by Anthropic; they are parsed as
   optional throughout.
3. **The shell tail poller against a live shell.** Tailing is unit-tested
   against files on disk, and the poller's start/stop is tested against the
   shell set, but no test runs a real background `Bash` call and watches the
   tail grow.
4. **Push invalidation end to end.** The pump marks the entity changed and the
   detail verbs carry `surfaces`, both tested separately. The full
   agent → revision bump → `entity.changed` → SPA refetch → repaint loop has
   not been exercised against a live bridge.
5. **Row actions actually reaching an agent.** The menu posts a canned message
   through the existing composer path; the message text and wiring are tested,
   but no test confirms an agent received one and acted on it.
6. **Entry counts.** Field *lengths* are bounded; the *number* of subagents,
   shells, or checklist items in a snapshot is not. Each entry is now on the
   order of a kilobyte, so this only matters for an agent that spawns
   thousands, which the spec does not address.

### Security

Re-scored against
[`planning/v2/Issue Security Checklist.md`](planning/v2/Issue%20Security%20Checklist.md):
**100/100**, controls 1, 2 and 4 re-run by name and the rest inside the green
suite. One finding was opened and fixed during this pass — the unbounded
free-text fields described above. Beyond the checklist's boundary, the new
data path holds:

- The workflow script and the subagent prompt never reach the snapshot
  (`a_started_workflow_never_holds_the_script_it_was_handed`,
  `a_started_subagent_never_holds_the_prompt_or_the_call_that_spawned_it`).
- A running shell's output path stays in the ledger and never rides the wire
  (`a_running_shells_output_path_is_the_ledgers_alone_and_never_the_snapshots`).
- No internal field is ever written (`no_snapshot_ever_writes_an_internal_field`).
- Every viewer escapes model-supplied text; hostile markup is pinned in
  `spa/test/agentSurfacesRender.test.js`.

`.gitleaks.toml` gained the two allowlist paths the tree actually has —
`planning/v1/*.md` (previously held only by line-numbered fingerprints, which
break when a doc shifts) and `bridge/target/`, the gitignored Cargo output
where the `pem` crate compiles its own doc-example RSA key into an rmeta.

### How to try it

```bash
cd bridge && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd spa && npm run lint && npm test
```

`spa/` lints for one thing only — function complexity over 10 (CLAUDE.md
“Complexity gates”). `npm test` (vitest) is the rest of its gate.

To see it in a browser, run the stack from **Run it locally** above, open an
agent on a worktree, and ask it for work that uses the harness's own machinery
— for example "run the test suite in the background", "spawn a subagent to
read the README", or "make yourself a checklist for this". Pills appear under
the status line as soon as the first one has something in it; click one to
open its viewer.

## 2026-09-14 — Multi-device: no current device

Branch `build/combined-interface`. Three staged packages of work
(`.build/plan/01-device-contexts.md`, `02-device-identity-routes.md`,
`03-retire-device-switcher.md`, against the design in
`00-multi-device-design.md`). The client used to be on one bridge at a time —
a device picker *switched* which machine the whole app was about, and
`App.session` / `App.call` / `App.cacheScope` / `App.chatRepository` were that
machine. It is now on all of them at once, and there is no current device left
to ask about.

### Shipped

- **A context per paired device** (`spa/src/core/deviceContexts.js`). One
  object per machine — its session and `rpc` (the one caller everything
  outside the registry holds, which reads whichever session that device is on
  now), its cache scope, its chat repository (drafts and controllers), its
  harness catalog, its offline mark —
  created when that device first answers and kept through reconnects, which
  only replace the transport. Three questions replace "which device is
  current": `homeContext()` (creation), `routeContext(App.route)` (a surface),
  `contextFor(row.deviceId)` (a row's verb). `canAnswer(context)` is the one
  wording of "can I ask this machine anything right now".
- **Every device open at once** (`spa/src/connection.js`). Boot opens a
  session to each online device; a device that comes up later joins without a
  reload; one going offline marks its own context and closes nothing else.
  Each device polls its own feed and the snapshots are merged
  (`core/feedMerge.js`), so the inbox and the projects rail are the account's
  work, not one bridge's. Rows carry `deviceId` and a `projectKey` minted
  account-wide (`core/deviceKey.js`), because every bridge mints a `proj-1`.
- **The picker filters** (`spa/src/devices.js`, `core/deviceFilter.js`).
  "All devices" or one machine, remembered under `build.deviceFilter`. It
  narrows the lists and nothing else: no route, no session and no creation
  reads it, and a surface open on a filtered-out machine stays open.
- **Links name their machine** (`core/router.js`):
  `#/device/<id>/project/<id>/branch/<name>`. A pre-redesign link with no
  device resolves by asking every live machine which one holds the entity
  (`views/resolving.js`) and rewrites itself.
- **A machine that cannot answer** — an arrival at a link whose device is
  offline or was never opened here mounts the device notice, which names the
  machine and hands the link back the moment it lands; a surface already open
  when its machine goes keeps its frozen-view treatment and gains a strip
  naming the device (`core/deviceNotice.js`). With no machine able to answer
  at all, the gate takes the app back and waits (`views/gate.js`).
- **Creation has one control**: Settings → **Creation device** ("New projects
  and captures go to"), written through `rememberSelectedDevice`. The
  composer's placeholder names it, captures queue against it by name, and
  **New project** in the rail is made there.
- **Per-device settings pages** (`views/deviceSettings.js`,
  `views/devicePanels.js`, `views/deviceProjects.js`): projects folder,
  projects list, Add project, Set remote, agent modes, default harness,
  isolation and triage — everything that is one bridge's answer, on that
  bridge's page, over its own session. The account page keeps what is the
  account's: creation device, agent defaults, appearance, notifications,
  downloads, devices and keys.
- **The aliases are gone.** `App.session`, `App.call`, `App.cacheScope`,
  `App.chatRepository`, `App.offline` and `App.offlineSince` left `App`, with
  `switchDevice`/`setHomeDevice`, `adoptApplicationScope`,
  `disposeApplicationScope` (its teardown survives as `resetApplication()`),
  the alias copiers, and `cacheScope.js`'s ambient accessors.
  `spa/test/noCurrentDevice.test.js` reads every file under `spa/src` with
  comments stripped and fails on any of those names, so they cannot come back.
- **Docs**: README's device sections and `planning/v2/UX Redesign
  Decisions.md` (dated lines at the inbox and router decisions) say what the
  product now does.

### Running the suites

```bash
cd spa && npm run lint && npm test && npm run build
```

Node 25 shipped an experimental Web Storage implementation, and it is a trap
for this tree: it defines a `localStorage` global that answers `undefined`
unless the process was started with `--localstorage-file`, and vitest's jsdom
environment leaves a global that is already defined alone. jsdom's own Storage
therefore never lands, and every jsdom suite dies on the first preference
`spa/src/app.js` reads. `spa/test/setup/localStorage.js` (wired in
`spa/vite.config.js` under `test.setupFiles`) installs an in-memory Storage
with `Object.defineProperty` — a plain assignment lands on Node's setter and
changes nothing — whenever there is no usable one, which also gives the
node-environment suites a `localStorage` they never had. The suite is green on
Node 22 (CI, `spa/.nvmrc`) and on Node 26, with or without
`--no-experimental-webstorage`.

### Unverified

1. **The two-bridge browser pass has not been run — for any of the three
   stages — and is still owed at the time of writing.** What it owes:
   two bridges online, both projects lists in one rail; filter to one machine;
   create a project and a capture and confirm both land on the creation
   device; stop one bridge — its rows grey and the other keeps working; stop
   both — the waiting screen; start one — the app returns without a reload.
   Everything claimed above is verified by the jsdom suite only
   (232 files / 4087 tests) plus lint, build, semgrep and gitleaks.
2. **Two real bridges of different releases.** The harness catalog is read per
   device precisely so two machines can offer different agents, but no test
   run has had two bridges built from different releases on one account.

### Follow-ons not done

1. **Bridge-minted globally unique project ids.** The client mints a
   `${deviceId}/${projectId}` key (`spa/src/core/deviceKey.js`) for its own
   lists and keys; the wire still carries the bare id each bridge minted, so a
   project id is only unique per machine.
2. **Account-wide capture routing.** A capture goes to the creation device and
   that machine's router answers it. Fan-in across machines is still a
   follow-on, as `UX Redesign Decisions.md` says.
3. **Per-device terminal sockets.** The shells are one socket that follows one
   machine at a time — the route's device, else home — and the WebRTC terminal
   channel rides that machine's link. Each device does upgrade its own app
   channel; only the terminal stream is single-machine.
4. **In-surface verb disabling.** Amendment 12's call was to keep the notice
   for an *arrival* at a machine that cannot answer, and to name the machine
   over a surface that was already open when its device went. Inside such a
   frozen surface the buttons are still live and refuse through the context's
   `rpc` rejecting; disabling each verb in place was not built.
5. **`spa/src/sheets/clone.js` was deleted** rather than migrated — nothing in
   the SPA opened it (the bridge's `project.clone` RPC is untouched).
