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
`rtc.offer`. The bridge does use the short-lived, per-user TURN username and
credential in that list: `offered_server` in `bridge/src/rtc.rs` copies them into
its ICE configuration. The long-lived Cloudflare TURN key they are minted from
stays on skriftapp and never reaches a browser or a bridge.

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

The daemon's configuration comes from environment variables. Most are read at
startup in `bridge/src/main.rs` and `bridge/src/config.rs` (`BridgeConfig`). A
few are owned by the module they tune and read there instead. Examples:
`BRIDGE_TERM_SHELL` in `bridge/src/terminal_environment.rs`,
`BRIDGE_CHILD_SCOPE` (child placement) in `bridge/src/priority.rs`, and the
`BRIDGE_ICE_*` policy in `bridge/src/rtc/policy.rs`. To find a variable, grep
for it.

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

**The design rule** is that the bridge is RPC plus push events. Product logic
belongs in the SPA. Logic that must run while no client is connected (agents,
watchers, presence, updates) belongs in the bridge as services isolated from
the RPC and event layer. New work should move toward that.

**Today the code does not fully follow the rule.** Some verbs still carry domain
logic in the request path. For example, `issues.assign`
(`bridge/src/api/v1/issues.rs`) calls `AppState::issues_assign`, and
`bridge/src/app/tracker/dispatch.rs` then applies the assignment policy: it
watches an issue assigned to the user, tracks the receiving agent, and links
the dispatch result, all inside the call. Read the handler before assuming a
verb is a thin read or write.

**Frames.** A request is `{id, method, params}`; a reply is
`{id, ok: true, result}` or `{id, ok: false, error, error_code, retryable, details}`.
A push is a frame with a `type` that answers no pending request. Pushes can carry
an `id`: the legacy `entity.changed` push is `{type: "entity.changed", id}`
(`ChangeKey::payload` in `bridge/src/changes.rs`). So the SPA's
`spa/src/core/sessionRpc.js` first matches `id` against its pending calls, and
treats a frame with a `type` that matches none as a push. Contract fixtures for
every verb live in `fixtures/api/v1/` and are checked by
`bridge/tests/api_contract.rs` and `spa/test/apiContract.test.js`.

**Admission.** `FrameIntake` in `bridge/src/carrier.rs` sees every frame first:

- It refuses non-signaling frames that arrive over the relay.
- It answers `ping` itself.
- It sends a receipt as soon as it admits a request.
- It answers signaling (`rtc.*`) and attachment writes at once, without
  queueing, so a negotiation never waits behind other work.

Everything else is queued to the worker pool in
`bridge/src/carrier/dispatch.rs`, which runs handlers on blocking threads.

**Dispatch.** Each handled frame goes to `dispatch_frame` in
`bridge/src/app/rpc.rs`, which tries these in order:

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

**Push events.** `ChangeBus` in `bridge/src/changes.rs` collects changes.
`ChangeBus::run` flushes the first change on an idle bus at once, then holds a
250 ms window (`DEFAULT_COALESCE_WINDOW`) open before the next flush. Changes
noted inside the window go out together when it closes. A
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
  `1.24.0`. `fixtures/api/versions.json` (`"current"`) must match it.
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
- A top-level param a v1 verb's params type does not declare is refused, never
  dropped: `invalid_params`, `unknown param: <name>`, with `details.params`
  listing every one (`parse_params` in `bridge/src/api/v1/mod.rs`; announced
  as `params.strict` since 1.24.0). It is the same answer an unknown kind
  gets from `changes.subscribe`. So a new param is announced like any other
  addition, and a client sends it only to a bridge whose greeting names it.
  Nested objects keep their own rules: a message context item's unknown
  field is still ignored, and its unknown kind refused.
- `PROTOCOL_VERSION` in `bridge/src/transport.rs` is a different number: the
  version of the E2EE envelope.

### Stores and persistence

- **SQLite**: `bridge/src/store.rs` opens `build.db` in WAL mode in the tasks dir
  (`BRIDGE_TASKS_DIR`, default `~/.build/tasks`). Tables are defined in
  `bridge/src/store/schema.rs`, and each area has its own module under
  `bridge/src/store/`. The upgrade sequence runs when the store opens, in
  `bridge/src/store.rs`: it adds hoisted columns, applies the schema,
  reclassifies stored items, and stamps `schema_version`.
  `bridge/src/store/migrations.rs` holds the column helper
  (`add_hoisted_column`) and the tests' old-version fixtures. The substantive steps live beside
  their tables: `migrate_agents_to_v6` in `bridge/src/store/entities.rs` and
  `ensure_operation_receipt_columns` in `bridge/src/store/operations.rs`. The
  store refuses to open a newer schema.
- **JSON files**: projects and settings in `~/.build/config.json`
  (`bridge/src/app/config/`), the device identity in `~/.build/identity.json`
  (`bridge/src/identity.rs`), workspace manifests `.build-workspace.json`
  (`MANIFEST_FILE` in `bridge/src/workspace.rs`), and update status and job files
  (`bridge/src/update/`).

### Workspaces and worktrees

`WorktreeManager` (`bridge/src/worktree/manager.rs`) is the entry point for
**git** checkouts. Its two isolation backends (`backends()`) live in
`bridge/src/isolation/`:

- `worktree.rs`: a git worktree created through libgit2 (`git2`), not the
  `git worktree` CLI.
- `rift.rs`: a copy-on-write checkout through the Rift CLI.

Sources with no git repository bypass the manager. The workspace code in
`bridge/src/app/workspaces/mod.rs` calls `copy_directory_with_rift_root` in
`bridge/src/isolation/directory.rs` directly. That function makes a plain copy
under worktree isolation. Under Rift isolation it makes a Rift snapshot, and it
falls back to a plain copy, recorded as a downgrade, when Rift is unavailable
or fails before writing anything.

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

Agent harnesses and the user's terminals are placed through
`bridge/src/priority.rs` as a `ChildKind` (`Agent` or `Terminal`). That covers
PTY children in `bridge/src/pty.rs` and the piped headless harnesses
(`bridge/src/harness/adk/session.rs`,
`bridge/src/harness/codex_app_server/process.rs`), all through
`ChildPlacement`. Short-lived git and isolation utility processes do not
go through it: `bridge/src/git_process.rs` spawns them directly, with a deadline.
On systemd, agents are placed in `app-build_agents.slice` (`AGENTS_SLICE`, CPU weight 20)
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

**The browser↔relay contract.** The relay is authentication and rendezvous and
nothing else
([`planning/v2/Strict P2P Transport Spec.md`](<planning/v2/Strict P2P Transport Spec.md>)).

1. `POST /api/gateway-token` (Skrift-session authed) returns `{token}` with a
   5-minute TTL (`GATEWAY_TOKEN_TTL` in `skriftapp/buildapp/devices_controller.py`).
2. `GET /api/devices` returns `[{device_id, approved, status,
   transport_public_key_b64, …}]`. Presence and transport keys come from here
   and only here; the relay reports neither. `status` is `online` iff the
   bridge's heartbeat landed within 90 s (`ONLINE_WINDOW` in
   `skriftapp/buildapp/presence.py`).
3. The browser opens `/ws/client` and sends `{"type":"authenticate","token":…}`
   first; the relay validates the token with the app and replies
   `{"type":"authenticated"}` or closes.
4. The client seals a fresh session key to the app-pinned device transport key
   and sends `session_init` with `route_to: "device:<id>"`; the device answers
   `session_accept`. One socket per device mints every session that device
   needs.
5. That socket then carries only E2EE envelopes whose inner method is `rtc.*`
   (offer, answer, trickled candidates). A bridge answers anything else with
   `error_code: "unavailable"`, `details: {reason: "relay_is_not_a_data_plane"}`
   and never dispatches it. The client closes the socket once both
   DataChannels are open and reopens it for an ICE restart or another mint.
6. Envelopes are opaque to the relay, which never holds a session key. A relay
   frame is capped at 64 KiB (`MAX_WS_MESSAGE_BYTES` in
   `bridge/src/relay_server.rs`).
7. Relay→app calls to `/internal/*` carry `X-Internal-Secret:
   $INTERNAL_API_SECRET`.
8. The bridge posts a device-signed `POST /api/devices/heartbeat` every 30 s.

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
| `spa/src/api.js` | the product API calls to skriftapp (`/api/devices`, `/api/gateway-token`, `/api/rtc/ice-servers`, push); the served-version check in `spa/src/core/version.js` is the other plain-HTTP read |
| `spa/src/devices.js` | device list, presence poll, device picker |
| `spa/src/core/` | the logic and renderers. Pure logic lives in `*Model.js`, painting in `*Render.js`, next to their mount controllers |
| `spa/src/core/bridgeApi/` | adapter selection and the v1 adapter |
| `spa/src/views/` | route surfaces, plus `gate.js` (boot) and `versionGate.js` |
| `spa/src/sheets/` | modal sheets (add device, repo browser, project/workspace settings, …) |
| `spa/src/terminal/` | ghostty-web terminal panes; `manager.js` owns the terminal session |
| `spa/src/styles.css`, `spa/src/styles/*.css` | tokens and styles |
| `spa/public/` | `theme-boot.js`, service worker, manifest, icons |

### Render from cache

**The rule.** All new SPA code follows it:

> Everything draws from the cache. A corollary is that nothing draws based on
> the status of a connection. All rendering assumes the local cache is up to
> date and is never aware of the connection state machine's status unless a
> render state showing that status is deemed necessary (generally never). If
> the device is connecting, everything renders from the local cache as if it
> is connected.

In practice:

- Pulls and pushes write to the cache, and the views holding those records
  redraw.
- No view waits on the wire to show what is already known.
- No view branches on whether a device is connected, connecting or away.

**How it is built:**

- **The cache** is `spa/src/core/localCache.js`: one IndexedDB database
  (`build-cache`) whose record keys are addresses of the form
  `deviceId|entityId|kind|sub`. It has `readCached`, `writeCached`,
  `writeCachedIfNewer`, `mergeCached`, `evictEntity` and friends.
  `subscribeCache(prefix, listener)` announces every write to listeners on a
  matching prefix, and a `BroadcastChannel` carries the announcements to other
  tabs.
- **The sync layer** is `spa/src/core/cacheSync.js`, the main reader of the
  wire. On a greeting, a reconnect or a tab return, `syncDevice()` makes one
  ordered pass per device: the lists, then the workspace being viewed, then the
  rest. Tabs share the Web Lock `build.cacheSync`, and the holder syncs for all
  of them. It holds three change subscriptions per device: `s-inbox`
  (realtime), `s-background` (git, files and shells on a 30 s cooldown) and
  `s-active` (the routed workspace, realtime).
- **Reconnect catch-up.** The bridge starts a subscription empty and records a
  change only for the subscriptions it holds when the change happens. So a pass
  takes its subscriptions out before it reads anything, and where the cache has
  something to show it waits (bounded) for the bridge to answer them. A device
  with nothing cached reads at once. Either way, a subscription the bridge
  takes on after a pass's reads were asked (`onSubscriptionHeld` in
  `spa/src/core/changeEvents.js`) is followed by another pass. A push can
  overtake a read, so a read never writes over a record a push wrote after the
  read was asked, nor under an entity a push took off the board since
  (`spa/src/core/pushFence.js`). A board read leaves a row a push wrote since
  unobserved in the `feed`, so the row's own record paints. Stopping sync
  permanently invalidates that lifetime's passes and push appliers, even on
  the same session. The first recovery pass refreshes file bodies still cached
  under live workspaces (at most `RECENT_FILES`, five per workspace), after
  reading their threads. Its completion is remembered per device, session and
  restored path until sync stops: ordinary feed refreshes and visible-tab
  passes add no body reads. Recovery remembers each file separately: successful
  reads stay settled while unknown failures get one retry on a later pass.
  A definitive missing-file refusal, or a second failure, leaves the last cached
  body in place and stops retrying until a matching files push or a new recovery.
  A push renews only the named paths (all held paths if truncated), including
  their retry budget; a pass cannot complete debt that a newer push reopened.
  Late background subscription coverage starts a new recovery that the earlier
  pass cannot complete. A subsequent pass still waiting for coverage takes
  over that recovery before it reads. This repairs abandoned file refreshes
  and changes missed while away or across a reload. Only the newest read of each
  file may write its answer; removing an entity still invalidates all its readers.
  A conversation's forward read (`syncThreadWindow` in
  `spa/src/core/threadSync.js`) only
  carries items made past the cursor. When its page shows the conversation's
  counter moved on a value none of its items wears, an item under the cursor
  changed in place. The record then owes a repair (`repairThrough`, written
  with the cursor) until the newest `REPAIRED_THREAD_ITEMS` (50) held items
  have been read again at or past that counter.
- **Pushes**: `watchChanges()` in `spa/src/core/changeEvents.js` registers
  subscriptions and routes each `changes` flush to the appliers (`APPLIERS` in
  `spa/src/core/cacheSync.js`). Most fields carry bodies that are written
  straight into the cache: `state`, `thread`, git `status`/`log`/`unpushed`/
  `diff`, the files root listing, and `terminals`.
- **Optimistic writes** also go into the cache first, and the push that follows
  confirms them.
- **Route surfaces** (branch, issue, tracker issue, project, workspace) and the
  shell's rail and console stand on `surfaceContext(route)`
  (`spa/src/core/surfaceContext.js`): the device's context, or a session-less
  one when only its records are on disk (a cold reload). They never ask whether
  the machine can answer. `mountDeviceNotice` (`spa/src/core/deviceNotice.js`)
  stands in only for a machine nothing here has ever held.
- Entity-specific caches sit beside it: `issueCache.js`, `trackerCache.js`,
  `conversationCache.js`, `surfacesCache.js` in `spa/src/core/`.

No timer polls the bridge for data. The data timers are the device presence
poll against skriftapp and the served-version check. The transport has its own:
the reconnect backoff (`spa/src/core/deviceRecovery.js`) and the peer's open
timeout and TURN-to-direct upgrade (`spa/src/core/peerLink.js`). Two more
touch data without polling for it: the persistence debounce in
`spa/src/core/localUiState.js`, which holds a UI-state edit for `debounceMs`
before writing it to IndexedDB, and the `run.adopt` re-asks in
`spa/src/core/adoption.js`, which ask again every `ADOPT_REASK_MS` (2 s), at
most `ADOPT_REASK_LIMIT` (30) times, while the bridge is still adopting a
checkout. The cross-tab sync-lock fallback (`LOCK_WAIT_MS` in
`spa/src/core/cacheSync.js`) is another, listed under Current exceptions below.
Others include the boot retry and cosmetic clocks.

**Current exceptions.** These describe today's code, not the rule. Don't copy
them into new code; each is a candidate to bring under the rule.

- **Surface-owned reads.** A few surfaces read on demand, for data the sync
  pass does not hold:
  - the Files tab lists a directory the reader expands with `fs.tree`
    (`spa/src/core/fileTree.js`);
  - the archive reads `archived.list` (`spa/src/views/archive.js`);
  - the agent rail reads `settings.get` (`spa/src/core/agentRail.js`).
- **Invalidation pushes.** Some push fields only say what moved, and the
  applier reads again:
  - `issues` carries only ids, so the project's issue list is re-read;
  - changed `files` paths re-list the directories the reader opened and
    re-read open file bodies with `fs.read`;
  - a git item that could not carry its diff pulls it for the routed
    workspace, or marks it stale.
- **Cross-tab fallback.** A tab not granted the sync lock within
  `LOCK_WAIT_MS` (4 s) syncs anyway, in case the holder is frozen, and a
  browser without the Locks API syncs every tab. The cost is duplicate reads.
- **Connection-aware rendering.** Some surfaces still show device state:
  - greyed "offline" rows and the strip over a surface whose machine is away
    and not being dialled, worded by `spa/src/core/deviceAway.js`;
  - the notice a link to a machine with no records here shows ("Connecting
    to …" or away), `spa/src/core/deviceNotice.js`;
  - the connection icon (`spa/src/connectionStatus.js`);
  - the compose box's "… is away" note, painted before anything is queued
    (`spa/src/core/composeView.js`, #137).
  - **pending, not accepted:** the account's offline mark
    (`accountSaysAway`, written by `markWhatTheAccountNoLongerLists` in
    `spa/src/devices.js`) lands only on contexts that exist at the presence
    read. A surface stood up on a machine's records after that read shows no
    strip until the next one.

### Connection state machine

`spa/src/connection.js` runs one **application** session per device, carried
only over that device's DataChannels. Terminals mint their own E2EE session
through the same device's rendezvous (`mintTerminalSession`), and it rides the
`term` channel.

- **Rendezvous**: `createRelayRendezvous` (`spa/src/core/rendezvous.js`) opens
  the relay socket, mints the session (`session_init` / `session_accept`) and
  carries `rtc.*`. Its lifetime is managed by `spa/src/core/rendezvousLifecycle.js`.
- **Peer**: `openPeerLink` (`spa/src/core/peerLink.js`) opens the `app` and
  `term` channels. It times out after 15 s, and it tries to move a session that
  has sat on TURN for 20 s onto a direct pair. `spa/src/core/transportPath.js`
  classifies the path.
- **Session**: `openSession` (`spa/src/core/session.js`) combines
  `spa/src/core/sessionRpc.js` (encryption, pending calls, receipts, pushes)
  with `spa/src/core/sessionSwitch.js`, which sends `rtc.*` over the rendezvous
  and everything else over the peer.
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
  `api_version` against `SPA_API_RANGE` (`>=1.2.0 <2.0.0`). A greeting with
  no version, or no greeting at all, reads as `PRE_ALPHA_API_VERSION`
  (`0.0.0`). For compatibility, that is matched at the lowest adapter's floor
  rather than rejected. Any other version outside the range returns
  `{unsupported: "bridge" | "app"}`, and `spa/src/views/versionGate.js` says
  which side needs updating.
- `capabilitiesOf()` in `spa/src/core/bridgeApi/v1/index.js` turns the greeting
  into feature flags. A greeting with a `capabilities` array (1.22.0 and later)
  is taken as-is. For older bridges (`>=1.0.0 <1.22.0`), the frozen
  `LEGACY_CAPABILITIES` table infers features from the minor version and
  greeting flags. New features get a name, never a legacy row.
- Surfaces read the flags with `bridgeCapabilities(deviceId)`
  (`spa/src/core/changeEvents.js`), which falls back to `NO_CAPABILITIES`.
- A flag that changes what a view draws is written to the cache at the
  greeting and read from there, so a cold mount draws what it will keep:
  `issues.commentUserNotifies` becomes the per-device Needs you rule in
  `spa/src/core/needsYouRule.js`, read by the Issues tab and the inbox's
  watched issues.
- A session is adopted before it is greeted, so `canAnswer` is true before the
  greeting's verdict. Each greeting, including a re-greeting on a new carrier
  or a restored path (`greetLiveBridge` in `spa/src/connection.js`), gets its
  own authority and arms `context.greeted`; the first hello claims the wait
  reserved at session adoption. A newer greeting supersedes every
  older one on that session, transferring pending waits to its promise; only
  the current authority can install an adapter or verdict and release the wait.
  `whenGreeted()` in `spa/src/core/deviceContexts.js` checks `stands()` and
  dispatches in the same turn: the context still owns the session, this is its
  latest issued greeting, and that greeting reported a compatible API. The
  model catalog and both project/workspace `ensure_conversation` requests use
  this authority, including owner creation through captured repository callers;
  an owner answer whose authority no longer stands is rejected too.
  The catalog checks it again when the answer arrives and inside the cache
  write transaction; a superseded answer is re-asked under the current greeting,
  or left unasked until a lost machine returns. Cached surfaces paint throughout.
  `user.present` likewise uses the greeting at dispatch and at its cache write,
  while retaining the arrival's freshness and focus checks.

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
| Files | `spa/src/views/files.js`, `spa/src/core/fileTree.js` (+ `fileTreeModel.js`), `spa/src/core/fileTabs.js` (+ `fileTabsModel.js`), `spa/src/core/fileViewer.js`, `spa/src/core/fileEditor.js` |
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
