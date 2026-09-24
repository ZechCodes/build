# Architecture

How Build's two main pieces are built: the **bridge** (`bridge/`, a Rust daemon on
the user's machine) and the **SPA** (`spa/`, the web client). This file describes
the code on `main`. The specs in [`planning/v2/`](planning/v2/) record intent and
can lag behind; where they disagree with the code, the code wins. Paths are
relative to the repository root.

## System topology

```
                    getbuild.ing                relay.getbuild.ing
┌─────────┐  HTTPS ┌────────────┐  /internal/*  ┌────────────┐  wss ┌─────────┐
│ browser │◄──────►│ skriftapp  │◄──────────────│ Rust relay │◄────►│ bridge  │
│  (SPA)  │        │ api + SPA  │ X-Internal-   │ ciphertext │      │ (user's │
└──┬─┬────┘        └─────┬──────┘    Secret     │ rendezvous │      │  box)   │
   │ │                   ▼                      └────────────┘      └────┬────┘
   │ │             Postgres 16                        ▲                  │
   │ └────── wss /ws/client (while negotiating) ──────┘                  │
   └╌╌╌╌╌╌╌ WebRTC DataChannels (direct; Cloudflare TURN fallback) ╌╌╌╌╌╌╌┘
```

| Piece | Where in the tree | Runs on |
| --- | --- | --- |
| SPA | `spa/` (built into `skriftapp/buildapp/static/`) | the browser, or inside the desktop app |
| skriftapp | `skriftapp/buildapp/` | getbuild.ing (`deploy/k8s/app.yaml`) |
| relay | `bridge/src/bin/relay.rs`, logic in `bridge/src/relay_server.rs` | relay.getbuild.ing (`deploy/k8s/relay.yaml`) |
| bridge | `bridge/src/main.rs`, library in `bridge/src/lib.rs` | the user's machine |
| desktop app | `desktop/` (Electron; loads the hosted SPA, see `desktop/src/security-policy.mjs`) | the user's desktop |
| E2EE protocol | separate repo `build-secure-transport`: the SPA imports its JS binding; the bridge carries a Rust port in `bridge/src/transport.rs`, checked by `bridge/tests/interop_python.rs` | both ends |

A session goes like this:

1. The SPA signs in to skriftapp and asks it for a gateway token and the device
   list (`spa/src/api.js`). Presence and each device's transport key come from
   skriftapp, never from the relay.
2. The SPA opens `/ws/client` on the relay, seals a session key to the device's
   transport key, and negotiates WebRTC over that socket with `rtc.*` messages
   (`spa/src/core/rendezvous.js`).
3. Two negotiated DataChannels open, `app` (id 0) and `term` (id 1)
   (`NEGOTIATED_CHANNELS` in `bridge/src/rtc.rs`, `spa/src/core/peerLink.js`). The
   SPA then closes the relay socket. Every RPC, push and terminal byte from then
   on goes peer to peer, directly or through TURN.
4. The bridge refuses application traffic that arrives over the relay: the
   relay is a rendezvous, not a data plane (`bridge/src/carrier.rs`). A device
   that cannot be reached directly or through TURN is shown as blocked. There is
   nothing to fall back to.

The browser gets its ICE server list from skriftapp (`POST /api/rtc/ice-servers`,
`skriftapp/buildapp/ice_servers.py`) and hands it to the bridge inside the sealed
`rtc.offer`, so the bridge holds no TURN credentials.

---

## The bridge

One Rust crate: `bridge/Cargo.toml` builds the `build-bridge` binary
(`bridge/src/main.rs`) on the `build_bridge` library (`bridge/src/lib.rs`), plus
the relay binary `bridge/src/bin/relay.rs`.

### Process and subcommands

`main()` in `bridge/src/main.rs` dispatches on the first argument:

| Subcommand | What it does |
| --- | --- |
| `serve` (default) | runs the daemon |
| `mcp --task <owner>` | the stdio MCP server an agent's harness launches (`mcp_stdio`) |
| `pair` | device-initiated pairing: prints a code and waits for approval in Build (`bridge/src/pairing.rs`) |
| `install-service` / `uninstall-service` | systemd user unit or launchd agent (`bridge/src/service/`) |
| `update-helper` | the detached self-update helper (`bridge/src/update/installer.rs`) |
| `provision`, `backup` | print an identity bundle; online SQLite backup |
| `--version` | prints the version |

Every `BRIDGE_*` environment variable is read in `bridge/src/main.rs` and
`bridge/src/config.rs`.

### Module layout

Modules are declared in `bridge/src/lib.rs`. The main groups (paths relative to `bridge/src/`):

| Area | Modules |
| --- | --- |
| RPC and state | `app.rs` + `app/` (`AppState`, `app/rpc.rs` dispatch, one submodule per area: `board`, `captures`, `config`, `conversations`, `git`, `issues`, `projects`, `rtc`, `runs`, `runtime`, `tracker`, `updates`, `watchers`, `workspaces`, `worktrees`) |
| Wire contract | `api/` (`API_VERSION`, capabilities, `ApiError`), `api/v1/` (typed verbs, one file per family) |
| Transport | `carrier.rs` + `carrier/` (relay socket or DataChannel behind one boundary; worker pool), `transport.rs` (E2EE), `relay.rs` (relay client), `rtc.rs` + `rtc/` (WebRTC peer, ICE policy, chunking), `liveness.rs`, `presence.rs`, `reachability.rs` |
| Push | `changes.rs` (`ChangeBus`) |
| Persistence | `store.rs` + `store/` (SQLite), `identity.rs`, `workspace.rs` (manifests) |
| Checkouts | `worktree.rs` + `worktree/` (`WorktreeManager`), `isolation/` (git worktree, Rift, plain copy), `lifecycle/`, `watch.rs`, `diff.rs`, `gitgui/`, `git_process.rs` |
| Agents | `harness/` (providers), `pty.rs`, `screen.rs`, `agent.rs`, `thread/`, `delivery.rs`, `reaper.rs`, `resume.rs`, `priority.rs`, `mcp.rs` + `mcp/` |
| Work model | `orchestrator/`, `plan.rs`, `run.rs`, `branch.rs`, `capture.rs`, `router.rs`, `tracker.rs`, `attention.rs`, `operation.rs` |
| Services | `update/` (self-update), `service/` (install), `notify.rs` (web push) |

### RPC and push events

The bridge is RPC plus push events. It keeps no product logic behind the RPC
layer beyond what must run while no client is connected (agents, watchers,
presence, updates). Those run as services that the RPC handlers read from and
signal, not inside the request path.

**Frames.** A request is `{id, method, params}`; a reply is
`{id, ok: true, result}` or `{id, ok: false, error, error_code, retryable, details}`.
A push is a frame with a `type` and no `id`. Contract fixtures for every verb live
in `fixtures/api/v1/` and are checked by `bridge/tests/api_contract.rs` and
`spa/test/apiContract.test.js`.

**Dispatch.** `bridge/src/carrier/dispatch.rs` runs frame handlers on blocking worker
threads. Each frame goes to `dispatch_frame` in `bridge/src/app/rpc.rs`, which
tries in order:

1. `signaling()`: `rtc.offer`, `rtc.ice`, `rtc.close`, answered without the app
   lock (`bridge/src/app/rtc.rs`).
2. the update-admission gate (`bridge/src/update/admission.rs`).
3. `session_scoped()`: verbs that need the caller's own `SessionSender` or the
   shared state (`session.hello`, `bridge.stats`, `term.*`, `agent.attach`,
   `agent.start`, `agent.interrupt`).
4. `routed()` → `AppState::route` → `crate::api::v1::dispatch`, then the legacy
   arms (`ping`, `term.list`, `term.close`, `stream.*`).

**The verb registry** is `bridge/src/api/v1/mod.rs`. Each family file (`board.rs`,
`changes.rs`, `git.rs`, `issues.rs`, `lifecycle.rs`, `thread.rs`, `updates.rs`,
`workspace.rs`) has a `methods()` table built with the `v1_method!` macro, which
names the verb, its handler and its typed params and result. Handler signatures
never take `serde_json::Value`; a test in that module enforces it. Slow git work
is deferred and runs with the lock released.

**Push events.** `ChangeBus` in `bridge/src/changes.rs` collects changes and
flushes them after a 250 ms coalescing window (`DEFAULT_COALESCE_WINDOW`). A
session that greeted with `changes: "subscriptions"` gets `changes` frames only
for what it subscribed to with `changes.subscribe`. The events the bridge
announces are `ANNOUNCED_EVENTS` in the same file (`board.changed`,
`entity.changed`, `changes`, `bridge.update_status`). Terminal output
(`term.output`, `term.reset`, `term.closed`) comes from `bridge/src/screen.rs`.
Example pushes are in `fixtures/api/v1/events.json`.

### App state and its lock

`AppState` (`bridge/src/app.rs`) holds the daemon's state. It is shared as
`Arc<std::sync::Mutex<AppState>>` via `AppState::shared()`. It is a std mutex, so
holding it across an `.await` is denied crate-wide
(`#![deny(clippy::await_holding_lock)]` in `bridge/src/lib.rs`, which explains
the wedge that motivated it). Parts that must not wait on the lock have their
own: WebRTC peers sit behind an `RwLock` (`PeersSlot` in `bridge/src/app/rtc.rs`),
and `Store` wraps its own connection mutex (`bridge/src/store.rs`).

### Liveness and signaling

`LivenessRuntime` (`bridge/src/liveness.rs`) is a small, separate tokio runtime
whose threads are named `bridge-live`. Nothing on it may take the app lock.
`run_daemon` in `bridge/src/main.rs` puts three things on it: the relay socket,
the presence reporter, and the WebRTC peer factory. That way a busy app lock
cannot starve the relay connection, the heartbeat or negotiation.

- **Presence** (`bridge/src/presence.rs`): a device-signed heartbeat to
  skriftapp's `/api/devices/heartbeat` every 30 s (`HEARTBEAT_INTERVAL`), sent
  only while the relay socket is authenticated (`bridge/src/reachability.rs`).
- **Signaling**: the `rtc_offer`, `rtc_ice` and `rtc_close` handlers in
  `bridge/src/app/rtc.rs`. The ICE policy is `bridge/src/rtc/policy.rs`, set by
  `BRIDGE_ICE_POLICY`, `BRIDGE_ICE_RELAY_MIN_WAIT_MS` and `BRIDGE_ICE_INTERFACES`.
  Large messages are split and reassembled by `bridge/src/rtc/chunk.rs`.

### Wire versioning and capabilities

- `API_VERSION` in `bridge/src/api/mod.rs` is the wire version, currently
  `1.22.0`. `fixtures/api/versions.json` (`"current"`) must match it.
- `session.hello` is answered by `session_hello` in
  `bridge/src/app/runtime/terminals.rs`. The reply carries `api_version`,
  `capabilities`, `push_events`, `events` and the `changes` subscription settings.
  It also records what the client said it was (`bridge/src/api/clients.rs`).
- `capabilities` comes from `capabilities()` in `bridge/src/api/mod.rs`: every
  registered v1 verb by its exact name, the legacy verbs (`LEGACY_METHODS`) and
  the named cross-verb features (`FEATURE_CAPABILITIES`, e.g. `agents.names`,
  `changes.subscriptions`, `errors.codes`), sorted. The SPA reads features off
  this list. It never probes.
- Rule, from the header of `bridge/src/api/v1/mod.rs`: additive changes in one
  release share one minor bump. Only a breaking removal or shape change needs a
  new major.
- `PROTOCOL_VERSION` in `bridge/src/transport.rs` is a different number: the
  version of the E2EE envelope.

### Stores and persistence

- **SQLite**: `bridge/src/store.rs` opens `build.db` in WAL mode in the tasks dir
  (`BRIDGE_TASKS_DIR`, default `~/.build/tasks`). Tables are defined in
  `bridge/src/store/schema.rs`, migrations are in `bridge/src/store/migrations.rs`,
  and each area has its own module under `bridge/src/store/`. The store refuses
  to open a newer schema.
- **JSON files**: projects and settings in `~/.build/config.json`
  (`bridge/src/app/config/`), the device identity in `~/.build/identity.json`
  (`bridge/src/identity.rs`), workspace manifests `.build-workspace.json`
  (`MANIFEST_FILE` in `bridge/src/workspace.rs`), and update status and job files
  (`bridge/src/update/`).

### Workspaces and worktrees

`WorktreeManager` (`bridge/src/worktree/manager.rs`) is the single entry point
for checkouts. It hands out isolation backends from `bridge/src/isolation/`:

- `worktree.rs`: a git worktree created through libgit2 (`git2`), not the
  `git worktree` CLI.
- `rift.rs`: a copy-on-write checkout through the Rift CLI.
- `directory.rs`: a plain copy, for sources with no git repository.

A workspace brings together one checkout per project source. Its manifest is
`.build-workspace.json`, and they are indexed by `WorkspaceRegistry`
(`bridge/src/workspace.rs`). Creation and deletion live in
`bridge/src/app/workspaces/` and `bridge/src/lifecycle/`. Work branches are named
`build/<slug>`. `bridge/src/watch.rs` runs one filesystem watcher per checkout,
and those feed git and files changes into the `ChangeBus`.

### Harnesses and the agents' slice

`harness_for()` in `bridge/src/harness/mod.rs` is the one place a provider
(`AgentProvider` in `bridge/src/models.rs`) is turned into a `Harness`:

| Provider | Module | Shape |
| --- | --- | --- |
| `Claude` | `bridge/src/harness/claude.rs` | Claude Code TUI in a full PTY (`bridge/src/pty.rs`) |
| `Codex` | `bridge/src/harness/codex.rs` | Codex TUI in a PTY |
| `ClaudeAdk` | `bridge/src/harness/adk.rs`, `bridge/src/harness/adk/` | Claude Code headless over its stream-json session protocol |
| `CodexAppServer` | `bridge/src/harness/codex_app_server/` | Codex's app-server JSON protocol |
| `Pi` | `bridge/src/harness/pi.rs` | Pi, with the `build-tools.ts` extension |

Every child the bridge spawns goes through `bridge/src/priority.rs`. On systemd,
agents are placed in `app-build_agents.slice` (`AGENTS_SLICE`, CPU weight 20)
and the user's terminals in `app.slice`, through a transient unit started over
`busctl`. Where that is unavailable, it falls back to `nice`. `install-service`
sets the slice's weight (`bridge/src/service/systemd.rs`). The result is that
the bridge outranks the user's apps, and those outrank the agents.

### MCP tools

Each agent gets Build's MCP server as `build-bridge mcp --task <owner>`.
`DoneServer` in `bridge/src/mcp.rs` speaks newline-delimited JSON-RPC over stdio
and forwards each call to the daemon's Unix socket (`BRIDGE_MCP_SOCKET`, default
`<worktrees>/build-bridge-mcp.sock`). The daemon side is `spawn_done_socket` and
the handlers in `bridge/src/app/mcp.rs`.

The tools an agent sees depend on its surface (`McpSurface`: `Coding`, `Router`,
`Project`). Each action is a `BridgeAction` variant. The tool lists are
`coding_tools()`, `router_tools()`, `project_tools()`, `workspace_tools()` and
`issue_tools()`. They cover conversation tools (`post_thread_message`,
`message_agent`, `set_topic`, `compact_self`, …; compaction in
`bridge/src/mcp/compaction.rs`), workspace tools (`create_workspace`,
`add_workspace_agent`, …) and the tracker (`get_issue`, `comment_issue`,
`move_issue`, `label_issue`, …).

### Relay and direct connection

- **Relay client**: `bridge/src/relay.rs` holds a `wss` connection to
  `/ws/device` (default `wss://relay.getbuild.ing`, `DEFAULT_RELAY_URL` in
  `bridge/src/config.rs`) and authenticates with the device's Ed25519 identity.
  `relay_forever` in `bridge/src/main.rs` reconnects it with backoff
  (`bridge/src/backoff.rs`).
- **Relay broker**: `bridge/src/bin/relay.rs` and `bridge/src/relay_server.rs`
  serve `/ws/device`, `/ws/client` and `/health`. They validate against skriftapp
  over `/internal/*` and forward only opaque envelopes.
- **Direct path**: `bridge/src/rtc.rs` on the `webrtc` crate answers the
  browser's offer and carries the `app` and `term` channels. `carrier.rs` puts
  both carriers behind one `FrameIntake`/`SessionSender` boundary, so handlers
  never know which one a frame came in on.

### Update and install

- `bridge/src/update/`: `UpdateService` (`service.rs`) checks the public releases
  repo (`release.rs`) and verifies the signed `SHA256SUMS`. `installer.rs` runs a
  detached helper (via `systemd-run` on Linux) that swaps the binary, watches the
  new one on probation, and rolls back if it fails. Dev builds only report and
  never install (`provenance.rs`). The SPA reads the status through the
  `bridge.update_status` push and the verbs in `bridge/src/api/v1/updates.rs`.
- `bridge/src/service/`: `install-service` writes the `build-bridge.service`
  systemd user unit (`systemd.rs`) or a LaunchAgent (`launchd.rs`).
- `scripts/install.sh` is the public installer. It verifies and installs the
  binary, then runs `build-bridge pair` and `build-bridge install-service`.

### Bridge tests

Unit tests sit beside the code (`#[cfg(test)]` modules, and directories such as
`bridge/src/app/tests/` and `bridge/src/store/tests/`). Integration tests are in
`bridge/tests/`: the wire contract (`api_contract.rs`), relay (`relay_*.rs`),
WebRTC (`rtc_peer.rs`), presence (`presence.rs`, `device_presence.rs`), E2EE
interop (`interop_python.rs`, which needs a `build-secure-transport` checkout),
store migration and the complexity ratchet (`complexity_ratchet.rs`). The
complexity threshold is in `bridge/clippy.toml`.

---

## The SPA

Vanilla JavaScript ES modules with no framework, bundled by Vite
(`spa/vite.config.js`, `base: "/app/static/"`). The build output goes to
`skriftapp/buildapp/static/`, which skriftapp serves at `/app/`. Every
dependency is self-hosted. `@build/secure-transport` is a `file:` dependency on
a sibling `build-secure-transport` checkout (`spa/package.json`).

### Module layout

| Path | What it holds |
| --- | --- |
| `spa/src/main.js` | entry: fonts, CSS, theme, router, device picker, then `boot()` from `spa/src/views/gate.js` |
| `spa/src/app.js` | the `App` object, route handling (`go`, `initRouter`), the `VIEWS` table, `render` |
| `spa/src/connection.js` | per-device session lifecycle |
| `spa/src/api.js` | the only plain-HTTP calls, all to skriftapp (`/api/devices`, `/api/gateway-token`, `/api/rtc/ice-servers`, push) |
| `spa/src/devices.js` | device list, presence poll, device picker |
| `spa/src/core/` | the logic and renderers. Pure logic lives in `*Model.js`, painting in `*Render.js`, next to their mount controllers |
| `spa/src/core/bridgeApi/` | adapter selection and the v1 adapter |
| `spa/src/views/` | route surfaces, plus `gate.js` (boot) and `versionGate.js` |
| `spa/src/sheets/` | modal sheets (add device, repo browser, project/workspace settings, …) |
| `spa/src/terminal/` | ghostty-web terminal panes; `manager.js` owns the terminal session |
| `spa/src/styles.css`, `spa/src/styles/*.css` | tokens and styles |
| `spa/public/` | `theme-boot.js`, service worker, manifest, icons |

### Render from cache

**Every view paints from the cache.** Pulls and pushes write to the cache, and
the views holding those records redraw. No view waits on the wire to show what
is already known, and none calls the bridge for a read.

- **The cache** is `spa/src/core/localCache.js`: one IndexedDB database
  (`build-cache`) whose record keys are addresses of the form
  `deviceId|entityId|kind|sub`. It has `readCached`, `writeCached`,
  `writeCachedIfNewer`, `mergeCached`, `evictEntity` and friends.
  `subscribeCache(prefix, listener)` announces every write to listeners on a
  matching prefix, and a `BroadcastChannel` carries the announcements to other
  tabs.
- **The one reader of the wire** is `spa/src/core/cacheSync.js`. On a greeting,
  a reconnect or a tab return, `syncDevice()` makes one ordered pass per device:
  the lists, then the workspace being viewed, then the rest. Only one tab syncs
  (Web Lock `build.cacheSync`). It holds three change subscriptions per device:
  `s-inbox` (realtime), `s-background` (git, files and shells on a 30 s
  cooldown) and `s-active` (the routed workspace, realtime).
- **Pushes**: `watchChanges()` in `spa/src/core/changeEvents.js` registers
  subscriptions and routes each `changes` flush. Flushes carry record bodies, so
  they are written straight into the cache.
- **Optimistic writes** also go into the cache first, and the push that follows
  confirms them.
- Entity-specific caches sit beside it: `issueCache.js`, `trackerCache.js`,
  `conversationCache.js`, `surfacesCache.js` in `spa/src/core/`.

The only timers are the device presence poll against skriftapp, the served
version check, the boot retry and cosmetic clocks.

### Connection state machine

`spa/src/connection.js` runs one session per device, carried only over that
device's DataChannels:

- **Rendezvous**: `createRelayRendezvous` (`spa/src/core/rendezvous.js`) opens
  the relay socket, mints the session (`session_init` / `session_accept`) and
  carries `rtc.*`. Its lifetime is managed by `spa/src/core/rendezvousLifecycle.js`.
- **Peer**: `openPeerLink` (`spa/src/core/peerLink.js`) opens the `app` and
  `term` channels. It times out after 15 s, and it tries to move a session that
  has sat on TURN for 20 s onto a direct pair. `spa/src/core/transportPath.js`
  classifies the path.
- **Session**: `openSession` (`spa/src/core/session.js`) combines
  `spa/src/core/sessionRpc.js` (encryption, pending calls, receipts, pushes)
  with `spa/src/core/sessionSwitch.js`, which sends `rtc.*` over the rendezvous and everything else
  over the peer.
- **States**: a device's lifetime is `spa/src/core/deviceLifecycle.js` (`new` →
  `available`; unavailable as `away` / `blocked` / `refused`; `retired`). Each
  attempt is `spa/src/core/deviceConnectionAttempts.js` (`idle`, `connecting`,
  `succeeded`, `failed`). The backoff is `spa/src/core/deviceRecovery.js`. The UI
  shows the projection from `spa/src/core/connectionStatusModel.js`
  (`connected`, `attempting`, `waiting`, `offline`). Per-device contexts live in
  `spa/src/core/deviceContexts.js`.

A surface calls the bridge through its device context: `context.rpc(method,
params)` from `contextFor(deviceId)` or `routeContext(route)`. `context.rpc`
refuses when that device cannot answer.

### Capability gating

- `hello()` in `spa/src/core/changeEvents.js` sends `session.hello` with the
  client's `api_range`. `greetBridge()` then selects an adapter and arms the
  change subscriptions.
- `selectAdapter()` in `spa/src/core/bridgeApi/index.js` checks the bridge's
  `api_version` against `SPA_API_RANGE` (`>=1.2.0 <2.0.0`). When it falls
  outside, it returns `{unsupported: "bridge" | "app"}`, and
  `spa/src/views/versionGate.js` says which side needs updating.
- `capabilitiesOf()` in `spa/src/core/bridgeApi/v1/index.js` turns the greeting
  into feature flags. A greeting with a `capabilities` array (1.22.0 and later)
  is taken as-is. For older bridges (`>=1.0.0 <1.22.0`), the frozen
  `LEGACY_CAPABILITIES` table infers features from the minor version and
  greeting flags. New features get a name, never a legacy row.
- Surfaces read the flags with `bridgeCapabilities(deviceId)`
  (`spa/src/core/changeEvents.js`), which falls back to `NO_CAPABILITIES`.

### Surfaces

Routing is hash-based: `spa/src/core/router.js` parses and builds routes, and
`VIEWS` in `spa/src/app.js` maps a route kind to its renderer. Settings routes
open as a modal (`spa/src/views/settingsModal.js`).

| Surface | Files |
| --- | --- |
| Inbox | `spa/src/views/inbox.js`, `spa/src/core/inboxShell.js`, `spa/src/core/inboxView.js`, `spa/src/core/inbox.js` |
| Conversation and agent rail | `spa/src/core/shell.js`, `spa/src/core/agentRail.js` (+ `agentRailModel.js`, `agentRailRender.js`), `spa/src/core/chatRepository.js`, `spa/src/core/thread*.js` |
| Issues list, board, dashboard | `spa/src/views/projectView.js` → `spa/src/core/trackerIssuesPane.js`; `trackerListRender.js`, `trackerBoardRender.js`, `trackerDashboardRender.js` in `spa/src/core/` |
| Issue page | `spa/src/views/trackerIssueView.js` → `spa/src/core/trackerIssuePage.js` |
| Changes and git | `spa/src/core/gitPane.js`, `gitRender.js`, `changesReview.js`, `changesModel.js`, `changesRender.js`, mounted from `spa/src/views/workspaceView.js` and `spa/src/views/branchView.js` |
| Files | `spa/src/views/files.js`, `spa/src/core/fileViewer.js`, `spa/src/core/fileEditor.js` |
| Terminal | `spa/src/core/console.js`, `spa/src/terminal/` |
| Settings | `spa/src/views/settingsModal.js`, `settings.js`, `deviceSettings.js`, `devicePanels.js` in `spa/src/views/`; `spa/src/sheets/` |

### Theming

Tokens are CSS custom properties in `spa/src/styles.css`: `:root` holds the light
theme and `:root[data-theme="dark"]` the dark one. `spa/src/core/theme.js` is
the only code that sets `data-theme`, and `spa/public/theme-boot.js` sets it
before first paint.

- `--accent` is the foreground and affordance colour (text, icons, borders). It
  is `#006d43` in light and `#51ffb4` in dark.
- `--accent-fill` is a background: `#51ffb4` in both themes, always paired with
  `--accent-ink` text (`.btn.primary`, `.badge`). Never use it as a foreground.

### SPA tests

Vitest is configured in the `test` block of `spa/vite.config.js`, with two
projects:

- `unit`: `spa/test/**/*.test.js` outside `spa/test/browser/`. Suites that touch
  the DOM declare `@vitest-environment jsdom`. `fake-indexeddb` stands in for the
  cache.
- `layout`: `spa/test/browser/*.test.js`, run in real Chromium through
  `spa/test/browser/layoutHarness.mjs` (`playwright-core`, Chromium on `PATH` or
  `CHROMIUM_PATH`).

`npm test` runs both; `npm run test:browser` runs only `layout`. Setup files are
in `spa/test/setup/`. `spa/eslint.config.js` has one rule, `complexity` max 10,
whose ratchet count is pinned by `spa/test/complexityRatchet.test.js`.

---

## Where to look

| Task | Files |
| --- | --- |
| Add an RPC verb | a handler and typed params/result in `bridge/src/api/v1/<family>.rs`, listed in that file's `methods()` with `v1_method!`; a fixture `fixtures/api/v1/<verb>.json` (`bridge/tests/api_contract.rs` fails without one). The capability is announced automatically. Verbs that need the caller's session go in `session_scoped()` in `bridge/src/app/rpc.rs` and `LEGACY_METHODS` in `bridge/src/api/mod.rs` instead |
| Call a new verb from the SPA | `context.rpc("ns.verb", params)` from `spa/src/core/deviceContexts.js`; write the result into the cache (`spa/src/core/localCache.js`, or read it in `spa/src/core/cacheSync.js`) and have the view `subscribeCache`; gate it on its name in `capabilitiesOf()` (`spa/src/core/bridgeApi/v1/index.js`) and `NO_CAPABILITIES` (`spa/src/core/changeEvents.js`) |
| Add a push event | emit through `ChangeBus` (`bridge/src/changes.rs`); on the SPA side add it to `EVENT_TYPES` (`spa/src/core/bridgeApi/v1/index.js`) and its dispatcher in `spa/src/core/changeEvents.js`; example in `fixtures/api/v1/events.json` |
| Bump the wire version | `API_VERSION` in `bridge/src/api/mod.rs`, `"current"` in `fixtures/api/versions.json`, `fixtures/api/v1/session.hello.json`, and the new fixtures' `since`. A named feature goes in `FEATURE_CAPABILITIES` (same file) |
| Add a view | a route in `spa/src/core/router.js`; `spa/src/views/<name>.js`; register it in `VIEWS` in `spa/src/app.js`; rail or console via `spa/src/core/shell.js`; styles in `spa/src/styles/`; tests in `spa/test/` |
| Add an MCP tool | a `BridgeAction` variant with its `tool_name()` and `surfaces()` arms and a schema in the right `*_tools()` list in `bridge/src/mcp.rs`; handle it in `bridge/src/app/mcp.rs` (or `bridge/src/app/conversations/` for thread actions) |
| Add a harness | a `Harness` impl in `bridge/src/harness/`, an `AgentProvider` variant in `bridge/src/models.rs`, and its arm in `harness_for()` |
| Change what an agent runs under | `bridge/src/priority.rs`, `bridge/src/service/systemd.rs` |
| Change a colour | tokens in `spa/src/styles.css` (both themes) |
| Change transport or ICE | `bridge/src/rtc.rs`, `bridge/src/rtc/policy.rs`, `spa/src/core/peerLink.js`, `spa/src/connection.js` |
