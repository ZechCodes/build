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
| `pair` | device-initiated pairing: prints a code, the short fingerprint and an approve link (`<web>/app/#/pair/<code>`), all within 80 columns, and waits for approval in Build (`bridge/src/pairing.rs`); an identity stored as approved that the api's status answer reports `revoked` or `unknown` (or, from an older api, just not approved) is renamed to `identity.json.retired-<device id>` and a new identity is paired in its place, when that answer comes from the api that approved it (`approved_by` in the identity file, recorded only when a pairing through that api completes, else the default api; urls compared by scheme, host and effective port) or `pair --retire` says so; another api's answer ends `pair` with the identity untouched and exit status 3, which `install.sh` answers with `pair --retire` |
| `install-service` / `uninstall-service` | systemd user unit or launchd agent (`bridge/src/service/`); both gate on the same status answer as `pair`, and uninstalling keeps the identity file and says so |
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

The processes the daemon starts inherit its environment, with two exceptions
for agent sessions (PTY harnesses, the ADK session, the codex app server) and
CLI version probes: they drop a parent agent's session markers
(`INHERITED_AGENT_MARKERS`) and the daemon's identity (`DAEMON_IDENTITY_VARS`:
`BRIDGE_IDENTITY_FILE` and env-provisioned keys), both in
`bridge/src/harness/mod.rs`. A user's own terminal tab keeps the identity, so
`build-bridge pair` typed there finds the identity the daemon uses (#320).

### Module layout

Modules are declared in `bridge/src/lib.rs`. The main groups (paths relative to `bridge/src/`):

| Area | Modules |
| --- | --- |
| RPC and state | `app.rs` + `app/` (`AppState`, `app/rpc.rs` dispatch, one submodule per area: `board`, `captures`, `config`, `conversations`, `git`, `tasks`, `projects`, `rtc`, `runs`, `runtime`, `tracker`, `updates`, `watchers`, `workspaces`, `worktrees`) |
| Wire contract | `api/` (`API_VERSION`, capabilities, `ApiError`), `api/v1/` (typed verbs, one file per family) |
| Transport | `carrier.rs` + `carrier/` (relay socket or DataChannel behind one boundary; worker pool), `transport.rs` (E2EE), `relay.rs` (relay client), `rtc.rs` + `rtc/` (WebRTC peer, ICE policy, chunking), `liveness.rs`, `presence.rs`, `reachability.rs` |
| Push | `changes.rs` (`ChangeBus`) |
| Persistence | `store.rs` + `store/` (SQLite), `identity.rs`, `workspace.rs` (manifests) |
| Checkouts | `worktree.rs` + `worktree/` (`WorktreeManager`), `isolation/` (git worktree, Rift, plain copy), `lifecycle/`, `watch.rs`, `diff.rs`, `gitgui/`, `git_process.rs` |
| Agents | `harness/` (providers), `pty.rs`, `screen.rs`, `agent.rs`, `thread/`, `delivery.rs`, `reaper.rs`, `resume.rs`, `priority.rs`, `mcp.rs` + `mcp/` |
| Work model | `orchestrator/`, `plan.rs`, `run.rs`, `branch.rs`, `capture.rs`, `router.rs`, `tracker.rs`, `attention.rs`, `operation.rs` |
| Services | `update/` (self-update), `service/` (install), `notify.rs` + `notify/` (web push for what adds to the unread counter, #191, its content sealed to each browser's notification key, #200; see [Sealed push content](#sealed-push-content)), `reclaim.rs` + `reclaim/` (workspace reclaim, run by `app/workspaces/reclaim.rs`) |

### RPC and push events

**The design rule** is that the bridge is RPC plus push events. Product logic
belongs in the SPA. Logic that must run while no client is connected (agents,
watchers, presence, updates) belongs in the bridge as services isolated from
the RPC and event layer. New work should move toward that.

**Today the code does not fully follow the rule.** Some verbs still carry domain
logic in the request path. For example, `tasks.assign`
(`bridge/src/api/v1/tasks.rs`) calls `AppState::tasks_assign`, and
`bridge/src/app/tracker/dispatch.rs` then applies the assignment policy: it
watches a task assigned to the user, tracks the receiving agent, and links
the dispatch result, all inside the call. Read the handler before assuming a
verb is a thin read or write.

**Frames.** A request is `{id, method, params}`; a reply is
`{id, ok: true, result}` or `{id, ok: false, error, error_code, retryable, details}`.
A push is a frame with a `type` that answers no pending request. The SPA's
`spa/src/core/sessionRpc.js` first matches `id` against its pending calls
(replies and admission receipts), and treats a frame with a `type` that
matches none as a push. Contract fixtures for
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
`changes.rs`, `git.rs`, `push.rs`, `tasks.rs`, `lifecycle.rs`, `thread.rs`, `updates.rs`,
`workspace.rs`) has a `methods()` table built with the `v1_method!` macro, which
names the verb, its handler and its typed params and result. Handler signatures
never take `serde_json::Value`; a test in that module enforces it. Slow git work
is deferred and runs with the lock released.

`conversation.reset` belongs to the typed thread family. It reserves one exact
conversation, stops its processes and stages files outside the app lock, then
commits the replacement under the lock. Cleanup does not depend on a browser
staying connected. It takes the
project, entity, agent, canonical conversation and expected thread generation
along with an optional replacement harness/model/effort choice. A mismatched
address or stale generation is refused before any history or process changes.

**Push events.** `ChangeBus` in `bridge/src/changes.rs` collects changes.
`ChangeBus::run` flushes the first change on an idle bus at once, then holds a
250 ms window (`DEFAULT_COALESCE_WINDOW`) open before the next flush. Changes
noted inside the window go out together when it closes. Every session gets
`changes` frames only for what it subscribed to with `changes.subscribe`. A
greeting that omits `changes: "subscriptions"` is still accepted and gets the
same subscriptions session: the legacy invalidation pushes went in 3.0.0. The
events the bridge announces are `ANNOUNCED_EVENTS` in the same file
(`changes`, `bridge.update_status`, `models.changed`). Terminal output
(`term.output`, `term.reset`, `term.closed`) comes from `bridge/src/screen.rs`.
Example pushes are in `fixtures/api/v1/events.json`.

### App state and its lock

`AppState` (`bridge/src/app.rs`) holds the daemon's state. It is shared as
`Arc<std::sync::Mutex<AppState>>` via `AppState::shared()`. It is a std mutex, so
holding it across an `.await` is denied crate-wide
(`#![deny(clippy::await_holding_lock)]` in `bridge/src/lib.rs`, which explains
the wedge that motivated it). No tokio worker waits on it either: a task that
needs the lock (the pumps in `bridge/src/app/runtime/pumps.rs`, the MCP control
socket in `bridge/src/app/mcp.rs`, the idle monitor, the terminal reaper, the
update checks, the workspace reclaim service's start and sweeps, an off-lock
job's apply phase) takes it inside
`off_the_workers` (`bridge/src/app/runtime/off_the_workers.rs`), which runs the
section on the blocking pool. A worker parked on the lock behind a slow frame
would stop the runtime's I/O driver; `bridge/src/app/tests/runtime/off_the_workers.rs`
holds that line on a one-worker runtime. Parts that must not wait on the lock have their
own: WebRTC peers sit behind an `RwLock` (`PeersSlot` in `bridge/src/app/rtc.rs`),
and `Store` wraps its own connection mutex (`bridge/src/store.rs`).

### Liveness and signaling

`DedicatedRuntime::liveness()` (`bridge/src/liveness.rs`) is a small, separate
tokio runtime whose threads are named `bridge-live`. Nothing on it may take the
app lock. `run_daemon` in `bridge/src/main.rs` puts three things on it: the
relay socket, the presence reporter, and the WebRTC peer factory. That way a
busy app lock cannot starve the relay connection, the heartbeat or negotiation.

`DedicatedRuntime::push()` (`bridge-push`) carries what the bridge pushes to
its clients, which is CPU rather than waiting: the change bus's flusher (a frame
serialized and encrypted per subscriber per window) and every terminal's byte
pump (a vt100 parse per chunk, a frame per attached client every 10 ms).
`AppState::with_push_runtime` hands it to the flusher and, through the session
registry, to each tab's `TabPumps`. With none set (the tests) both run on the
runtime that starts them.

- **Presence** (`bridge/src/presence.rs`): a device-signed heartbeat to
  skriftapp's `/api/devices/heartbeat` every 30 s (`HEARTBEAT_INTERVAL`), sent
  only while the relay socket is authenticated (`bridge/src/reachability.rs`).
  `Reachability` is a watch channel, and the beat also goes out the moment the
  socket becomes authenticated, so a just-started or reconnected bridge is
  online within a second rather than an interval (#321).
- **Signaling**: the `rtc_offer`, `rtc_ice` and `rtc_close` handlers in
  `bridge/src/app/rtc.rs`. The ICE policy is `bridge/src/rtc/policy.rs`, set by
  `BRIDGE_ICE_POLICY`, `BRIDGE_ICE_RELAY_MIN_WAIT_MS` and `BRIDGE_ICE_INTERFACES`.
  Large messages are split and reassembled by `bridge/src/rtc/chunk.rs`.

### Wire versioning and capabilities

- `API_VERSION` in `bridge/src/api/mod.rs` is the wire version, currently
  `3.12.0`. `fixtures/api/versions.json` (`"current"`) must match it.
  1.24.0 carried `workspaces.lifecycle`, `params.strict`,
  `branches.finishDelete` and `changes.refusedKinds`; 1.25.0
  `workspaces.reclaimBranches`, `settings.workspaceLifecycle` and
  `tasks.listPaged` (`limit`/`cursor` on `tasks.list`,
  `bridge/src/app/tracker/pages.rs`); 1.26.0 adds `bodies.pages` (`range`
  on `fs.read`, `git.diff`, `git.show` and `git.changeset_diff`,
  `bridge/src/body_page.rs`); 1.27.0 adds `tasks.createdUserMentions`:
  an agent's `create_task` can mark its `created` timeline event with optional
  `mentions_user`, making the watched task need the user until it is read.
  1.28.0 adds `board.conversationSessions`: a conversation's feed row carries
  its own `session_started_ms` and `last_activity_ms` (the project agent's
  inbox row is ordered by them, #103). 1.29.0 adds `tasks.unreadCounts`:
  a watched task on `tasks.list` and `tasks.get` carries `unread_count`,
  the count its inbox row says, which the Tasks tab badges and the rail read
  (#104). 1.30.0 adds `thread.attachmentChunks` (`offset`/`length` on
  `thread.attachment`) and `fs.mediaRawPages` (`range.raw` on `fs.read` for
  exact image, audio and video byte pages through 64 MiB). 2.0.0 is the task
  rename (#190), the first break: every tracker and plan verb and feature
  name moved to `tasks.*` or `task.*`, so a 1.x bridge and a 2.x SPA gate
  each other as out of date. The modules and fixtures keep their `v1` names.
  2.1.0 adds `push.registerKey` and `push.revokeKey` (sealed push content,
  #200). 2.2.0 adds `models.installedCli`: `models.list` (and `list_harnesses`)
  offer only what each harness's installed CLI runs, each provider carrying
  `cli_name`, `cli_version` and `unavailable` (the models that CLI is too old
  for, with `requires_cli`), and a `models.changed` push says to ask again
  (#203; see Harnesses and the agents' slice). 3.0.0 is the pre-release
  cut (#207), the second break: it removes the verbs no client called (every
  `plan.*` alias, the retired planning mutations on `task.*` and `run.*`,
  and unused reads and writes such as `branch.get`, `git.branches`,
  `tasks.link` and `worktree.create`), the legacy `board.changed` and
  `entity.changed` pushes, and the old task scheduler. The historical plan
  reads (`task.get`, `task.stages`, `task.doc`, `task.stage_doc`,
  `task.stage_diff`) stay for stored plans, and so do the `run.*` verbs an
  adopted worktree's review uses. A removed verb answers `unknown_method`.
  3.1.0 adds `agents.createdBy` (#216): an agent made by another agent's
  Build MCP call (`add_workspace_agent`, or `assign_task` to a new agent or
  workspace) carries `created_by` on its digest. Only the bridge's own
  callers set it; `agent.add` refuses the param. The SPA reads it off the
  cached rows (`spa/src/core/agentLineage.js`), on a machine whose greeting
  named the capability (see Capability gating): the creator's Agents panel
  lists those Build agents apart from its harness sub-agents, and an agent
  counts as running while any agent in its panel runs, transitively
  (`spa/src/core/agentLineageModel.js`).
  3.2.0 adds `project.update_source` (#228), which edits one source's label,
  base branch, remote and (past the first) folder. See Projects and sources.
  The SPA's settings sheet offers those edits only on a machine whose
  greeting names the verb.
  3.3.0 adds `sources.syncBase` (#267): each source row carries `sync_base`
  and `sync` (what the last sync of its base concluded),
  `project.update_source` takes `sync_base`, and `project.sync_source` asks
  for a sync now. See Projects and sources.
  3.4.0 adds `workspace.measure_sizes` (#273), which queues a size walk
  for the workspaces named (or all of them, or a project's) and answers at
  once; each size lands on the row as `lifecycle.size_bytes` with
  `lifecycle.size_measured_at_ms`. See Workspaces and worktrees.
  3.5.0 adds `updates.replaceDevelopmentBuild` (#322): the update status
  carries `can_replace_development_build` and `running_from_cargo_target`,
  and `bridge.install_update` takes `replace_development_build`. See Update
  and install. The SPA reads the status field off the cached status, so an older bridge (no field) gets the
  install script command instead of an Install it would refuse.
  It also adds `to` to a `moved` `task_action` on an agent's conversation
  message (#323), naming the destination column. Older messages omit it.
  3.6.0 adds `tasks.review.snapshot`, `tasks.review.get`, `tasks.review.diff`
  and `tasks.review.complete` (#328). Each verb is its own capability; review
  comments and Git actions arrive separately. Stale review mutations answer
  `stale_version`.
  3.7.0 adds `tasks.reviewComments` (#329): `tasks.comment` and MCP
  `comment_task` accept optional `anchor` (snapshot, directory, path, side,
  line), `reply_to` and `opinion` (snapshot and approve/request_changes verdict).
  These are ordinary task comments, with their metadata in the existing JSON
  comment record; old anchors keep their original snapshot identity.
  3.8.0 adds `tasks.review.act` (#330): per-source Merge and Push steps against
  saved commits. `tasks.review.get` also returns available destinations and
  persisted action results. The action capability is independent of review
  reads, comments and explicit completion.
  3.9.0 adds `fs.projectSources` (#359): `fs.tree`, `fs.read` and
  `fs.write` accept `project_id` plus `source_id`, resolving one configured
  source of that project. The pair is exclusive with workspace, run and
  worktree selectors. Existing project-only and workspace scopes keep their
  behavior; all source reads and writes use the same path containment checks.
  3.10.0 adds `tasks.bodyPrecondition` (#347): `tasks.update` accepts optional
  `expected_body_hash` alongside `body`, the lowercase hexadecimal SHA-256 of
  the exact UTF-8 bytes of the saved body. The bridge compares it under the
  app lock before applying any field changes. A mismatch refuses the whole
  update with nonretryable `stale_body`; the client rolls back its optimistic
  tick and refreshes the task. Existing callers may still omit the hash. The
  SPA sends it only to a machine whose cached greeting names the capability.
  3.11.0 also adds `rtc.candidateDiagnostics` and the `rtc.diagnostics` push
  (#369), reporting remote host/mDNS/srflx/relay counts and resolution reasons
  without addresses. These are additive changes in the same unreleased minor.
  3.12.0 adds `rtc.clientLanCache`, the optional paired `client_id` discovery
  hint on `rtc.offer`, and actual per-host connectivity-check diagnostics (#372).
  3.11.0 adds `conversation.reset` (#358) and thread generations on conversation
  digests and responses. Generation-aware requests refuse a cleared thread;
  the reset capability gates the menu, its generation-aware cache handling,
  and the added request parameters on existing conversation verbs.
  The SPA's adapter claims `>=2.0.0 <4.0.0`: it calls nothing a 2.x bridge
  lacks (what 2.x added after 2.0.0 is capability-gated), so the app can
  roll before the bridge.
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
  as `params.strict` since 1.24.0). So a new param is announced like any
  other addition, and a client sends it only to a bridge whose greeting names
  it.
- An unknown push kind refuses the whole `changes.subscribe` the same way:
  `invalid_params`, "Build cannot subscribe to <kind>: this bridge does not
  know it.", with `details.kinds` listing every unknown one (`known_kinds` in
  `bridge/src/api/v1/changes.rs`; announced as `changes.refusedKinds` since
  1.24.0). The SPA drops those kinds and re-subscribes once without them
  (`addDesired` in `spa/src/core/changeEvents.js`).
  Nested objects keep their own rules: a message context item's unknown
  field is still ignored, and its unknown kind refused.
- `PROTOCOL_VERSION` in `bridge/src/transport.rs` is a different number: the
  version of the E2EE envelope.

### Sealed push content

A browser push (#191) says what happened (#200): a task's `#N title` and the
first line of its news, or an agent's name and the first line of what it
said, with a deep link to the task or the conversation. The api, the push
service and every log see only ciphertext. The exact scheme, its failure modes
and its controls are
[`planning/v2/Push Content Security Checklist.md`](<planning/v2/Push Content Security Checklist.md>).

- The browser makes one non-extractable ECDH P-256 key per push subscription
  (`spa/src/pushKeys.js`), keeps it in IndexedDB for the service worker, and
  registers the public half with each bridge that announces
  `push.registerKey`, after every greeting (`spa/src/core/pushKeySync.js`).
  A subscription is named by `sid = b64u(SHA-256(endpoint))`, so the bridge
  never sees the endpoint and the api needs no new column.
- The bridge keeps the keys in `push_keys` (store schema 11,
  `bridge/src/store/push_keys.rs`, at most 32). `spawn_notify` builds the
  content (`bridge/src/notify/content.rs`, `bridge/src/app/board/agent_push.rs`,
  `bridge/src/app/tracker/push.rs`) and hands it to a spawned task. An
  agent's content leaves resolving its workspace's path on disk to that task.
  The task seals the content per key off the app lock within a 1 s budget,
  padded to a fixed 1024 bytes so no blob's length says which event happened
  (`bridge/src/notify/delivery.rs`, `bridge/src/notify/seal.rs`), signs a
  challenge that binds the sealed digest, and forgets the keys the api reports
  unknown. Any failure sends the #191 generic notify.
- The api (`skriftapp/buildapp/web_push.py`) checks only the shape of each
  blob, forwards it byte-identical to its subscription, and never decodes or
  logs it.
- The service worker (`spa/public/sw.js`) opens the blob (AAD-bound to sid,
  kind and entity id; freshness window; nonce replay store) or shows the
  generic copy. A click reaches an open window as a `build.push.open` message,
  which `spa/src/push.js` accepts only from this origin's service worker and
  only for an `/app/#/` link. A cold start opens the link with a `from=push`
  mark that the router takes off the URL (`takePushOpenMark`). Either way that
  one open lands the linked conversation on its latest message; every other
  `?agent=` link, a reload included, lands on the unread line.
- The sealing is not sender-authenticated: forgery is prevented only because
  the notification public key travels only over E2EE and never reaches the
  api.

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

**Projects and sources.** A project is one or more sources
(`ProjectSource`, `bridge/src/app/projects/mod.rs`), each a folder with a
label (`name`), the folder name it mounts under in workspaces (`mount`,
fixed once a source exists), a base branch, and whether it is a Git
repository. A project has no remote of its own. A source's remote is its
checkout's `origin`, read from the checkout for every row
(`ProjectSource::origin`, via git2 in process). `config.json` keeps no copy of
it, and ignores the copy older bridges wrote. The project row's `path`,
`base_branch` and `remote` are its first source's. That first source is the
project's home (orchestrator, registry identity) and cannot move.
A project whose home is not a Git repository is a valid project: its project
agent spawns and is delivered to there (`agent_launch_for`, not `orch_for`),
and only the verbs that use git (branches, diffs, git checkouts) refuse it with
"project is not a git repository", which the SPA's compose box turns into the
Initialize Git offer (`spa/src/core/gitInitializationOffer.js`, #297). Nothing
runs `git init` without the user asking.
A repository with no commits yet is a Git project too: `open_repo`
(`bridge/src/lifecycle/projects.rs`) takes its base from the unborn HEAD, or
the one given, and does not resolve it until something needs a commit.
The project's Files face (`#/project/<id>/files`) browses those original
sources directly, one collapsible root per source, using the workspace's
shared explorer. Files, tabs and folded roots have project-scoped records,
and each source has a separate directory cache. `fs.projectSources` gates
the explicit project/source selector; an older bridge can still browse the
first source through its legacy project-only scope.
An explicit toolbar return restores the project's last face from local UI
state (`spa/src/core/projectRailState.js`); ordinary project links still open
Tasks. Moving a source changes its Files cache namespace. An open draft stays
visible, but its captured source location must still match before any read or
save is dispatched, so moving a folder cannot redirect that draft's save.
`project.update_source` (`bridge/src/app/projects/source_update.rs`, git in
`bridge/src/lifecycle/source_update.rs`) edits a source in place:
- A new base branch must be a branch the checkout has.
- A new folder is held to what `project.add_source` holds one to.
- A new remote is written to the source's checkout with
  `git remote set-url`/`add`/`remove`. Each existing workspace checkout that
  has its own repository (a Rift or plain copy) and still names the old
  `origin` follows it; a worktree shares the source's config anyway. The
  answer's `checkouts_updated` counts them. A checkout git will not rewrite
  is not rolled back or hidden: `checkouts_failed` names it
  (`workspace_id`, `path`, git's `reason`), still on the old remote, and the
  source's own change stands.

**Keeping a base in step with its remote** (#267). A Git source whose
`sync_base` is on (the default for new sources and, unset, for existing ones:
`SYNC_BASE_FOR_NEW_SOURCES` and `SYNC_BASE_FOR_EXISTING_SOURCES` in
`bridge/src/app/projects/mod.rs`) has its base branch fetched and
fast-forwarded, never merged, rebased, reset or forced
(`bridge/src/source_sync.rs`). The fetch takes the one branch the base
follows (`branch.<base>.remote` and `branch.<base>.merge`, else `origin` and
the base's own name; `source_sync/upstream.rs`) and nothing else, no tags and
no submodules, from the url git resolves (rewrites applied) once it passes
`usable_remote_url`. Every git a sync starts runs as nobody's command
(`source_sync/git.rs`): unattended (`run_git_unattended` in
`bridge/src/git_process.rs`: no prompt of any kind, stdin closed, the process
group killed at its deadline), with no hooks (`core.hooksPath=/dev/null`: the
user did not start this merge, so their `post-merge` or
`reference-transaction` is not run for it) and no automatic maintenance
(`gc.auto=0`, `maintenance.auto=false`: it could detach and outlive the kill).
Filters stay on, since a checkout without git-lfs's smudge would write pointer
files, and run unattended like the rest. Where the base is checked out in the
source's own clean checkout, `merge --ff-only --no-overwrite-ignore` moves it
with its files, refusing to overwrite an untracked or ignored file. Only the
fetch is bound by the short deadlines: once started, the checkout is let
finish, capped at 10 minutes (`source_sync/checkout.rs`), since git killed
part-way through leaves `index.lock` and half the incoming files behind and a
filter such as git-lfs's smudge can make an honest checkout slow; past the cap
git is killed and the lock it took is removed. The files it had already
written stay as untracked files, and every later sync is skipped naming them
until the user removes them. Where the base is checked out
nowhere, a compare-and-swap `update-ref` moves the ref alone. Neither happens
while an operation on the base is part-way through in any worktree, read from
each worktree's git directory (`source_sync/in_progress.rs`): a rebase whose
`head-name` or a bisect whose `BISECT_START` names the base (a rebase detaches
HEAD, so its branch otherwise reads as checked out nowhere), or a merge,
cherry-pick or revert in the worktree whose HEAD is the base. An operation on
another branch, such as a merge stopped in an agent's workspace, does not
hold the base back. Local commits,
uncommitted changes, an operation in progress, the branch checked out in
another worktree, and a remote without the branch are reported and left
alone. Three things sync (`bridge/src/app/projects/base_sync.rs`): a service,
30 s after startup and then every five minutes, with the app lock released;
every workspace cut, first, for each source with the setting on (10 s, the
sources side by side, and the fetch skipped when one landed in the last
minute), which goes ahead and puts `warnings` on its answer for any base it
could not bring up to date, so an agent's `create_workspace` or `assign_task`
hears them; and `project.sync_source` (Sync now). The cut's 10 s bound the
whole sync: a cut fetches and moves a base checked out nowhere, but never
starts the checkout's fast-forward (`sync_base_for_a_cut`). A base checked out
in the source's own checkout and behind after the fetch is handed to the
service, which the cut asks to sync that source at once, and the workspace
branch is cut from the fetched commit instead of the base (#271), with no
warning. That needs the fast-forward to be one git would make: the cut reads
first, without writing anything, whether the checkout has uncommitted changes
or a file of the user's (untracked or ignored) where the remote now has one
(`refuse_what_would_stop`). If it does, the cut is taken from the base as it
stood, with a warning naming why. A remote that said it wanted a person (git
said it needed a password, or ssh could not use a key without its passphrase)
is left off the timer until one of the other two syncs it. A fetch that only
timed out is retried on the next pass, then after 10, 20, 40 and at most 60
minutes while it keeps timing out; a cut that ran out of its 10 s leaves the
source's status as it was. A security key waiting for a touch is one of
these timeouts: OpenSSH shows its notice only on a terminal, so Build cannot
tell it apart, and the key blinks for each attempt, about once an hour once
the backoff is at its longest, plus up to 10 s at each workspace cut. One sync runs per checkout at a
time. Each row's `sync` is kept in the store's `meta` table
(`bridge/src/store/source_sync.rs`), and a sync that lands notes the project
list changed.

`project.set_remote` is the same edit on the first source, kept for older
clients. Every remote a client names, on any verb, passes
`usable_remote_url` (`bridge/src/remote_url.rs`): no leading `-`, no
whitespace or control characters, no `transport::` helper, no ssh user,
host or port starting with `-` or holding a `%` (git decodes it), and only
https/http/ssh/git/file urls, `user@host:path` or an absolute path. Git is
handed a `--` before it.

A workspace brings together one checkout per project source. Its manifest is
`.build-workspace.json`, and they are indexed by `WorkspaceRegistry`
(`bridge/src/workspace.rs`). Creation and deletion live in
`bridge/src/app/workspaces/` and `bridge/src/lifecycle/`. Work branches are named
`build/<slug>`. `bridge/src/watch.rs` runs one filesystem watcher per checkout,
and those feed git and files changes into the `ChangeBus`.
Workspace Done and branch rows share an outcome computed from all runs on the workspace root, including runs hidden from the inbox.

**Reclaim** (#135) is a service, not a verb. `AppState::spawn_workspace_reclaim`
(`bridge/src/app/workspaces/reclaim.rs`) sweeps every managed workspace two
minutes after startup, then every hour. It also sweeps five seconds after a
task that links a workspace moves to Done or closes. A sweep reads each
workspace under the app lock, then measures it with the lock released
(`bridge/src/reclaim.rs`: `Subject::measure`). The measure covers the newest
activity (a conversation message, a commit, a file change outside `.git`,
`.build` and build output), dirty and unpushed counts, and the linked tasks.
Every walk spends from one budget per workspace (2 million entries, two
minutes, and the daemon's stop flag, `bridge/src/reclaim/budget.rs`). Git is
read in a process of its own, `build-bridge measure-git`, one repository at a
time (`bridge/src/reclaim/git_probe.rs`): libgit2's status, history walk and
diff cannot be interrupted, so the service kills and reaps the reading when
the budget runs out or the daemon stops. A measurement that runs out anywhere
(a Git reading, the activity walk, the size walk, or a budget found spent at
the end) is held as `unmeasured`: not idle, not reclaimable. A
sweep then takes the lock again to write the verdict. `workspace.list` rows
carry it as `lifecycle`. A workspace is idle after 24 h, or the device's
`workspace_idle_secs` setting (#167); `BRIDGE_WORKSPACE_IDLE_SECS`, where set,
overrides the setting. Each sweep reads the policy afresh
(`AppState::reclaim_policy_now`), so a change in Settings applies at the next
sweep, which the change asks for at once. What holds it: not ready, an agent working or a
terminal open anywhere inside it, uncommitted or unpushed work, a plain
directory, a linked task not Done, or tasks that could not be read.

The registry anchors reclamation to its configured managed storage directory.
Before measurement or cleanup, the workspace root and every manifest checkout
must match the registered root identity and remain inside that boundary,
without substituted symlinks. Reloads preserve the registered identity. Git readers
start from a pinned checkout directory. Pruning and trash removal use directory
descriptors, so replacing a pathname cannot redirect deletion into another
tree. Explicit reclaim carries the same boundary through its deferred removal.
An invalid boundary leaves the workspace unmeasured and preserves its files.

Each project agent gets one notice per sweep naming its newly quiet
workspaces, and the notice repeats daily while they stay quiet. The linked
tasks record `workspace_idle` and `workspace_pruned` as actor `build`,
without waking their trackers.

Dropping build output (tier 1) is off unless the device's `workspace_prune`
setting turns it on, or `BRIDGE_WORKSPACE_PRUNE` is set, which overrides it.
When it is on and nothing holds an idle workspace, the sweep reserves the
workspace under the lock (`reclaim_reserved`). While a workspace is reserved,
the delivery queue holds every turn for an agent inside it, and every bridge
write inside it answers `busy`: `term.create`, the `git.*` verbs that change a
tree or its refs, `fs.write`, `fs.mkdir`, attachments, `run.git_action`, Done,
removals, renames and directory changes. The sweep measures the Git state and
activity again and inspects the build output, with the lock released. A final
killable Git child reads one fresh index per repository and validates its
candidates together. The five-second final budget includes that child, taking
the lock again, cheap checks of holds, index and ignore-rule metadata, and
candidate paths, and each rename into the workspace's trash (`.build/reclaim`). Deadline checks inside the loops
stop further work, and any unfinished validation keeps the candidates and
marks the verdict `unmeasured`. The reservation lasts through the moves.
Configuration includes whose input files cannot be identified by libgit2's
configuration API keep their build output. Build output is an ignored,
untracked `node_modules`, `target`, `.venv` or `dist` inside one of the
workspace's checkouts, reached without a symlink and holding no repository of
its own (`bridge/src/reclaim/artifacts.rs`). The trash is emptied with the
lock released.

The bridge never removes a workspace on its own. `workspace.reclaim` (the
project agent's `reclaim_workspace`, `app/workspaces/reclaim/explicit.rs`)
refuses at once on the holds the app state knows, then reserves the workspace
and measures Git off the lock. A deferred write-back can hand the drain
another stage (`AppState::apply_deferred_stage`). The measurement is the
sweep's, on the same budget and the same killable reading, so it has ended
before the reservation's 15-minute backstop could. Once measured, reclaim reads
every hold again under the lock, reads Git once more on a short budget (five
seconds, `unmeasured` past it) so a commit that landed meanwhile still holds
the workspace, logs `workspace_reclaimed` on each linked
task without waking its trackers, and removes the workspace through the same
path as `workspace.delete`. That path stops every agent and terminal anywhere
under the workspace root first. Once the checkouts are gone, the drain deletes
the local branch each one carried (#167), through Done's `BranchDeletion`
(`app/workspaces/branch_delete/`): measured off the lock, deleted only at the
commit the checks passed, never a default branch, never a branch checked out
anywhere, never one with commits no remote has or whose remote cannot say which
branch is its default. A branch that has to stay never holds the reclaim up.
The answer names each repository's outcome (`deleted`, `kept`,
`restore_failed`, with the reason), and each task linking the workspace or
the branch records the same entry as `branch_deleted` or `branch_kept`, under
whoever reclaimed and without waking its trackers. The verdicts persist in the store's `meta`
table (`bridge/src/store/workspace_lifecycle.rs`).

**Sizes** (#273). A sweep sizes only a quiet workspace, so the Workspaces tab
asks for the rest: opening it calls `workspace.measure_sizes`, which queues a
walk per workspace and answers at once (`bridge/src/reclaim/sizes.rs`). A
workspace already queued or being walked is not queued again, and a size
measured within five minutes (`SIZE_REUSE_WINDOW`) is reused. The walker is
a thread of its own (`bridge-sizes`, `app/workspaces/sizes.rs`), niced like
an agent on Linux, that takes one workspace at a time: it reads the boundary
under the app lock, walks it with the lock released through the reclaim
service's own `size_within` and budget, then writes `size_bytes` and
`size_measured_at_ms` onto the lifecycle record (creating one with no
verdict if no sweep has run) and notes the workspace list changed. A walk
out of budget writes nothing, and the last size stands. A sweep keeps
whichever size was measured later (`LifecycleRecord::keep_newer_size`).
In the SPA the tab asks each time it opens, whether from a link to it or
from the Workspaces cell on the project's rail (`openTab` in
`spa/src/views/projectView.js`), through `spa/src/core/workspaceSizes.js` (at
once on a greeted machine that names the verb, else at its next greeting), and
paints from the cache: the greeting writes whether the machine measures
(`spa/src/core/workspaceSizeSupport.js`), and a row with no size yet shows a
dim dash there rather than a spinner.

### Task reviews

`bridge/src/reviews/` saves one task's workspace review as numbered snapshots.
`capture.rs` reads every manifest directory, resolves each Git directory's
committed HEAD and base (explicit override, configured base, upstream, empty
tree), and pins its nonempty base and head under private refs. Ref components
percent-encode bytes outside ASCII letters, digits, `_` and `-`: directory IDs
contain `:`, which Git cannot use in ref names. Metadata keeps the original IDs.
The snapshot ID is unique before any pins are written; `service.rs` saves the
metadata with a review version check and removes only the losing call's pins
at their expected OIDs. A snapshot from another workspace replaces the previous
snapshot history; its old pins are released after the metadata commits. Git
work runs through the deferred drain off the app
lock (`app/tracker/reviews.rs`).

Schema 13 stores review headers and snapshot metadata in `reviews` and
`review_snapshots` (`store/reviews.rs`), without patches or file bodies.
It adds `review_actions`, one row per selected source with its ordered step
results. Admission checks the review version and records running sources in
one transaction. Each step records its input and result before the next starts;
running rows prevent another action on that source until the operation ends.
Daemon recovery reclaims registered, owned temporary review checkouts for
unfinished sources, then marks their rows interrupted without replaying Git.
Ownership and child-process markers protect live or uncertain merge children;
temporary review checkouts are never reused as user target checkouts. Ordinary
database opens do not perform this recovery.
`read.rs` uses the recorded common Git directory and saved OIDs for commit
diffs, full tree listings and read-only, paged blobs, including unchanged files.
Diff listings cap file rows at 1,000 with `files_truncated`; patch reads retain
only the requested byte window, with path filters applied before rendering.
A removed linked worktree can still be read while its common repository exists;
a deleted repository is unavailable. Non-Git directories stay in the manifest
as live folders; existing scoped filesystem reads remain their reader.

Completion records the actor, description, review event and task Done in one
transaction. A changed column also gets a Moved event and the same post-write
behavior as an ordinary move. RPC and MCP share a 2,000-byte trimmed UTF-8
description limit. Task closure and workspace Finish remain separate. Ordinary moves
to Done complete an open review through the same writer with “Marked done”.
Snapshots change only review metadata. Agent Complete reports and merged
workspace Finish skip their old task-movement/closure hooks for review tasks.
Closing/completing and removing workspaces or projects retain review history
and refs; replacing its workspace or explicitly deleting task history releases
the previous pins. MCP exposes
`snapshot_review`, `get_review`, `read_review`, `act_review`, `complete_review` to coding and
project agents, authenticated and restricted to their own project.

`reviews/actions.rs` executes selected source steps off the app mutex and
publishes task invalidations after each persisted result. Targets resolve from
the source ID to the configured source repository. The service holds the same
configured-path `source_sync::SyncLock` as base sync while Git re-reads target
placement. `reviews/git_actions.rs` merges the saved head into a clean existing
target checkout or an owned temporary checkout on the target branch. Separate
repositories import only the saved OID under a private ref. Push is non-forced
to a configured remote and explicit branch, using the saved head or a recorded
merge tip. A Push retry can name the successful merge action; it never reruns
Merge. Merge disables commit signing and has a five-minute deadline; Push has
ten minutes, while reads retain thirty seconds. A merge timeout restores the
previously clean checkout only after verifying its state and any leftover lock.
A Push timeout is interrupted with an unknown outcome and a prompt to check the
remote. Destination reads cover only the latest snapshot's directories, so
retained history adds no Git reads to `tasks.review.get`. Results survive
completion and workspace replacement; an old result
still names its original snapshot. None of these operations finishes or removes
a workspace, stages source files, or requires a review opinion.

The task page embeds the saved review (`spa/src/core/taskReviewPage.js`), with
every saved directory and Changes/Files views. Workspace Changes can save a
review on a task (`workspaceReviewEntry.js`). Snapshot/base changes and explicit
completion live in `taskReviewControls.js`; assigning a reviewer uses the
ordinary assignee picker and a note naming the snapshot. Opinions and line
comments use `taskReviewFeedback.js` and stay in the task timeline, with no
reviewer or model restrictions. `taskReviewActions.js` offers separate Merge
and Push selections for each source, displays destinations and saved/live heads,
and keeps per-source results visible. Choices and submitted intent live in
`build-ui`; Git is sent only on an explicit action. Recorded success can finish
the selected steps' review after reconnect, while a failure or interruption
requires an explicit retry or an ordinary completion description.

`taskReviewSupport.js` holds the individually announced review verbs and the
comment feature in the device cache. `taskReviewCache.js` stores metadata under
the project and task, orders replies by review version and shared task read
order, and refetches stale writes without retrying mutations. Sync and task
invalidations refresh previously opened review records. The Changes and Files
adapters use distinct project-owned kinds and snapshot/directory/path subkeys;
Git trees/blobs come from the saved head and plain folders use exact saved
directory IDs with live filesystem reads. Body pages use the ordinary body
cache, with the file-head writer accepting review kinds. Failed refreshes remain
visible alongside held data. Selection, viewed marks and drafts use `build-ui`
and survive replica eviction and reconnects. Drafts belong to a task/snapshot;
viewed marks match task, directory, path and content key so unchanged files
remain viewed across snapshots.

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

**What the installed CLI runs** (#203) is `bridge/src/harness/installed/`. A
harness's `models()` is Build's curated catalog; `Harness::offer` is that
catalog seen through what its installed CLI said, and that is what
`models.list`, `list_harnesses` and the spawn gate use:

- Claude Code (`claude`, `claude_adk`) is asked `claude --version`, and every
  catalog row carries `min_cli`, the version whose changelog says "Added
  Claude <model>", cited beside it. An older CLI gets a 400 from the API on
  every turn, or runs the model at a 200k window it does not know better than,
  so the model is hidden and listed under `unavailable`.
- Codex (`codex`, `codex_app_server`) is asked its own list over a short-lived
  `codex app-server` (`initialize`, then `model/list` with hidden models),
  because only the CLI knows the account's list. The picker gets the unhidden
  models in the CLI's order; a session may start on any listed one. The curated
  list is the fallback when the CLI cannot list, or lists nothing Build could
  start.
- Pi is not asked, and offers its catalog whole.

`Readings` holds each CLI's last usable answer. A failed or abandoned refresh
keeps that answer without announcing a catalog change. Until a probe succeeds,
the curated catalog is the fallback. Catalog reads answer from what is held
and ask again on a background thread once a successful answer is ten minutes
old. Failed or incomplete answers retry from 15 seconds, doubling after each
failure up to the ten-minute TTL; success resets the backoff. Codex's version
alone is incomplete without a usable model list. The daemon checks held
readings every 15 seconds even when clients use their cached catalog, honoring
each CLI's retry delay. For direct installs and symlinks, a change to the PATH
hit's path, target or modification time bypasses the delay and starts retries
again at 15 seconds. Underlying updates behind unchanged mise shims or wrapper
scripts change none of those observed properties; they rely on periodic
refresh and session version hints. A session that reports a new version
differing from the held reading (the adk init line's `claude_code_version`,
codex's `initialize` `userAgent`) asks at once; repeated hints honor the retry
backoff. Dropped or panicking probe jobs release their claim. Attempts
are ordered so an older background result or abandoned job cannot overwrite a
newer spawn-gate reading; a spawn uses its own fresh answer. A retained answer
after a failed refresh cannot refuse a spawn on its own word. Each probe runs
the program by name with fixed arguments and no shell, from the home directory,
with a 3 s deadline, bounded output and its process group killed afterwards.
Stderr is
drained concurrently, keeping a 4 KiB tail of the last nonempty line and
logging at most 512 sanitized characters with a failure. Up to 50 ms of the
same deadline is reserved for the final stderr drain. Probes run with
`MISE_OFFLINE=1`, so a mise wrapper never starts an install that the deadline
would cut off halfway (`probe/child.rs`; `planning/v2/Installed CLI Probe Security Checklist.md`). A
changed answer is pushed as `models.changed` (`bridge/src/app/model_catalog.rs`).
A spawn is refused on a model the CLI cannot run, before anything is written
or started: `AgentSpawnPlan::probe_and_scaffold` (`bridge/src/delivery.rs`)
asks `refuse_unrunnable` with the `AppState`'s readings, which asks the CLI
again first when its answer is over 30 s old (a spawn runs off every RPC, so
it may wait out that one probe). The sentence says which version the model
needs; it is the agent's `start_error` and its conversation's `last_error`, and
the message that asked is settled as failed, not uncertain. `agent.add`
refuses such a model on a fresh answer alone, since an RPC cannot wait on a
CLI. A role (`role_models`) or the
project-agent setting that names such a model is a default rather than a pick:
the role passes to the next declared model, and the project agent starts on
the harness's own default.

### Conversation resets

An agent is a durable identity; its harness process and transcript are
replaceable. `conversation.reset` keeps the agent ID, canonical conversation
binding, name, roster position, workspace membership, task assignments and
trackers. It gives the conversation a new `Thread.id`, exposed as `thread_id`
on the agent digest and thread responses, and advances the canonical thread's
`thread_generation_revision`. Aliases share that revision; their independent
model-choice revisions do not order conversation generations. The generation
separates a fresh empty conversation from its predecessor even when both have
the same agent address and the replacement's sequence counter starts at zero.

Reset removes the old transcript, session lineage, revisions, operation
payloads, attachments, readings, compaction state and conversation summaries.
It stops every process bound to that conversation and retires their queued
work and MCP capabilities. An established running turn is stopped and reaped;
already admitted sends settle outside the app lock before history is replaced.
An unadopted spawn or delivery handoff is refused retryably. Reads remain
available during retirement, while conversation mutations are refused.
MCP authentication captures the source thread generation, and done reports
and agent actions validate it under the same lock as their writes. Deferred
task handoffs retain that source generation until settlement. A late callback
or a request carrying the old generation cannot restore the cleared content.
A new project-agent session uses the ordinary fresh-start scaffolding and
standing instruction templates.
After the first reset, conversation mutations require the current `thread_id`;
an older client that omits it receives a refusal rather than recreating a
retired payload. Internal service wake-ups capture the current generation
when new work is accepted under the app lock. Restart rosters retain the
canonical generation observed before disk and filesystem checks, so an old
roster cannot wake a cleared conversation. Initial-generation omissions and
ordinary reads retain their existing compatibility behavior.

The browser's conversation menu offers **Clear conversation** only when the
cached capability says that device supports `conversation.reset`. The
confirmation leads to the existing harness/model/effort chooser, prefilled
with the agent's settings. The final **Clear and start fresh** action sends
one reset; cancelling the chooser keeps the conversation. Detail and compact
settings are retained, and the empty conversation opens in the same place.

### MCP tools

Each agent gets Build's MCP server as `build-bridge mcp --task <owner>`.
`DoneServer` in `bridge/src/mcp.rs` speaks newline-delimited JSON-RPC over stdio
and forwards each call to the daemon's Unix socket (`BRIDGE_MCP_SOCKET`, default
`<worktrees>/build-bridge-mcp.sock`). The daemon side is `spawn_done_socket` and
the handlers in `bridge/src/app/mcp.rs`.

The tools an agent sees depend on its surface (`McpSurface`: `Coding`, `Router`,
`Project`). Each action is a `BridgeAction` variant. The tool lists are
`coding_tools()`, `router_tools()`, `project_tools()`, `workspace_tools()` and
`task_tools()`. They cover conversation tools (`post_thread_message`,
`message_agent`, `set_topic`, `compact_self`, …; compaction in
`bridge/src/mcp/compaction.rs`), workspace tools (`create_workspace`,
`add_workspace_agent`, …) and the tracker (`get_task`, `comment_task`,
`move_task`, `label_task`, …).

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
- **LAN discovery**: `bridge/src/rtc/mdns.rs` resolves browser UUID `.local`
  candidates on each addressed LAN interface, selecting multicast egress and
  membership explicitly and skipping container bridges, veth, loopback and
  VPN/tunnel or point-to-point interfaces. Answers must match the receiving
  LAN's subnet: Linux uses packet arrival metadata; other supported hosts use
  a conservative source/subnet check for private, link-local or CGNAT addresses.
  A shared wildcard receiver dispatches multicast and unicast replies to concurrent
  queries. `rtc/remote.rs` defers names from offers and trickle without holding
  negotiation, validates names before starting discovery, and retries transient
  failures twice. Only live lookups count toward the discovery limit. Work is
  cancelled when remote ICE credentials change or the peer closes; same-credential
  offers preserve it. A session keeps resolved addresses for 60 seconds across
  ICE restarts; fresh ICE credentials and ports still determine whether a cached
  address works.
- **Late direct checks**: the bridge opts into bounded ICE checks on waiting or
  in-progress non-relay pairs after TURN is selected. The selected path and its
  keepalives remain intact; successful alternatives do not nominate themselves.
  The browser's existing direct-pair monitor may then use its single optional
  ICE restart to select direct. Default behavior of the vendored library is
  unchanged unless its setting is enabled.
- **Unresolved-host conntrack sweep**: the bridge enables the vendored
  `SettingEngine` host-socket sweep; the library defaults to disabled. A valid
  unresolved UUID `.local` UDP host candidate starts a 250 ms grace, racing
  mDNS rather than waiting for discovery to fail. Scout and sweep packets also
  require matching IPv4 server-reflexive addresses in the current operational
  local and remote ICE candidate sets. Local evidence must still derive from a
  current advertised live host socket; terminal stats and previous remote
  generations cannot authorize work. Srflx equality is a traffic-avoidance
  heuristic for honest off-LAN peers, not a security boundary. The remote
  srflx is self-reported, and a malicious paired peer can copy the bridge's
  advertised srflx back. Distinct LANs behind CGNAT or a shared enterprise egress
  can also match. Missing evidence waits within
  the existing 25-second lifetime, then skips with `nat-evidence-missing`;
  unequal addresses yield `nat-address-mismatch` without values or packets.
  Later accepted trickle can establish a match before expiry. Host and relay
  addresses are not substituted for missing srflx evidence. On Linux/Tokio,
  neighbor scouting resolves on-link addresses without charging unresolved ARP packets
  to the ICE socket. Credential-free STUN Binding Indications then leave the
  advertised host socket for freshly usable neighbors on its actual interface
  subnet. `IP_PKTINFO` fixes the source/interface and `MSG_DONTROUTE` prevents
  off-link routing. Only wholly
  RFC 1918 or IPv4 link-local subnets with at most 1024 total addresses qualify;
  the bridge's addresses, network and broadcast are excluded. A container
  interface qualifies only when it owns the advertised host socket. Other
  platforms/runtimes skip the sweep with a fixed reason.
  The security controls are these private on-link subnet bounds, a candidate
  port floor of 1024 enforced by both bridge and vendor, the 200-packet/s ceiling,
  the 60-second interface lease, resolved-neighbor-only real probes,
  credential-free payloads, and authenticated-inbound-only candidate creation.
  Ports below 1024 cannot create a plan or consume its 32-port allowance;
  the floor does not exclude services on higher ports.
  Before each probe, Linux send-queue accounting reserves at least three
  quarters of `SO_SNDBUF` for ordinary ICE, DTLS and SCTP writes. A probe yields
  under pressure without advancing its destination; 200 packets/s is a ceiling,
  not a required rate. Ordinary ICE writes keep their existing path. A bounded,
  read-only snapshot identifies reachable, stale and delay neighbors on the
  owning interface. Before returning the bridge's answer, the active ICE
  generation records usable neighbors on the current owning interface as its
  baseline. That interface set and baseline are immutable for the generation:
  a later owner change cannot make an unobserved interface eligible for early
  probing. Resolving a port erases that port's destinations and closes scouts
  when none remain. The bounded baseline and eight-address admission membership
  survive a temporarily empty candidate-port set, so a later port with the same
  credentials still uses the pre-answer baseline without resetting its budget.
  They retire on credential change, direct selection, close or original plan
  expiry; retaining them does not extend the 25-second lifetime. For the first
  250 ms after an unresolved candidate arrives, a bounded snapshot poll runs
  every 20 ms, then every 100 ms. This reuses the existing
  read-only neighbor snapshot rather than adding a notification socket. At most
  eight distinct newly usable addresses per generation, shared by all candidate
  ports, may receive an early real-port indication before the ordinary 250 ms
  scout grace ends. On a busy LAN, unrelated newly usable neighbors can occupy
  those first eight slots; a ninth phone then loses early-grace priority. After
  the grace, the ordinary eligible real-port probe still precedes anonymous
  scouting, but scouts and a later restart may still be needed. Newly learned
  neighbors can also receive this baseline-absent priority after grace; the
  aggregate early-neighbor count can rise after scouting starts, without holding
  that started scout pass. Each send still requires current matching srflx
  evidence, a live owning host socket, approved subnet and destination, a fresh usable
  neighbor, source/interface pinning, packet and send-queue headroom, and active
  credentials. The early path uses the same credential-free indication and
  cancellation rules as every real probe. It neither trusts an ARP entry as peer
  identity nor creates an ICE candidate. Unknown destinations are scouted from
  at most five temporary
  sockets across the process, bound to that interface address at ephemeral ports.
  One zero byte to UDP discard port 9 causes ARP without credentials or session
  data. Scout sockets also yield at the quarter-buffer watermark. A scout pass
  orders addresses by distance to initially usable neighbors and the bridge's
  address, with stable ties.
  No new unresolved scout is admitted if observed incomplete neighbors plus
  unobserved reservations would exceed `min(256, gc_thresh2 / 2)`. A separate
  read-only `RTM_GETNEIGHTBL` guard admits no work at 75% of the actual global
  ARP table's `gc_thresh3`, including failed entries and other network namespaces.
  Closing a socket does not erase kernel neighbor state; numeric admission
  accounting stays conservative until a fresh observation. Neighbor addresses
  are used only for active work, without a persistent cache.
  A process-wide owning-interface key and first successful scout-enqueue timestamp
  reserve one scout pass per 60 seconds, including an in-progress or canceled
  pass. Source aliases, peers, generations and candidate ports share that reservation; neither
  completion nor restart renews it; known-neighbor-only work consumes no reservation.
  Later work probes only freshly usable kernel neighbors and does not re-scout
  unknown addresses inside the window. No
  neighbor knowledge is retained across sessions by this reservation. The registry
  holds at most 64 unexpired interface timestamps and skips new interfaces when
  full; it never evicts a live reservation. A fully
  sparse /22 pass produces about 3,000 ARP requests including default kernel
  retries, at most once per interface per window (about 50/s averaged over 60 s).
  Honest peers reporting unequal srflx addresses produce zero scout and sweep
  packets. This reduces
  broadcast load at the cost of deferring new unknown addresses during cooldown.
  Scout and real-probe traffic share a process-wide 200-packet/s ceiling;
  kernel ARP retries are measured separately by the namespace fixture.
  The driver accepts at most 32 distinct candidate ports and 32768 attempts per
  generation, and permits one
  repeat one second after a pass only while TURN still carries the connection.
  A successful early indication holds an unstarted scout pass until one second
  after the latest successful early indication, so the browser's authenticated
  check can arrive. An exact validated
  host/peer-reflexive tuple following that indication prevents scout start even if
  TURN is still selected, while preserving unresolved evidence for the existing
  optional restart. Absent that proof, the original bounded scout pass
  begins. A scout already started is not paused. A later STALE neighbor learned
  through a browser's unicast ARP refresh can move ahead in the real-probe order
  without restarting discovery. The process-wide 60-second interface lease
  begins only when a scout datagram is successfully enqueued, never when early
  observation or an early indication occurs.
  Every plan expires 25 seconds after candidate arrival. A complete pass requires
  subnet scouting and real-port probes to usable neighbors; a queued unresolved
  packet is not proof of delivery. Resolution, direct
  selection, credential changes and close cancel it and erase destination
  state and close scout sockets. Expiry also erases destination state; numeric
  upgrade eligibility and bounded local-subnet retirement markers survive so
  a late candidate port cannot start another discovery pass in that generation.
  Completed or expired unresolved work may still justify the existing optional
  fresh-generation restart while current NAT evidence matches. Missing or
  mismatched NAT evidence exposes zero eligibility even when historical probe
  counters are positive.
  A sparse subnet's tail may expire before a full pass, and direct selection
  stops a pass immediately. Each 28-byte indication
  has only a FINGERPRINT and a fresh random transaction ID, without credentials,
  ufrags or session identifiers; it requests no reply. Outbound packets open
  conntrack tuples so a browser's authenticated inbound STUN check can create
  a peer-reflexive pair through an inbound UDP DROP rule. No guessed address
  becomes an ICE candidate. Fixed-code `host-sweep` diagnostics report the
  generation, status, attempted/sent counts, aggregate eligible unresolved
  plans and whether a prflx pair followed, without addresses, ports or names.
  Three additive counts, `early_neighbors_probed`, `scout_holds` and
  `scout_starts`, make the early path and fallback visible without exposing an
  endpoint (`rtc.conntrackSweep`, additive within wire 3.12.0).
  Chromium 152 namespace measurements on a mostly-empty /22 completed scouting
  in 15–24.4 seconds, depending on global neighbor-table pressure. An unknown phone
  near a known neighbor received its first real-port probe 369 ms after the
  fixture proved its absence (419 ms after candidate gathering) and carried
  both encrypted pulls directly with zero
  restarts. An unknown far-edge phone took 11.6–19.0 seconds in coverage controls
  and 18.9 seconds in an initial production run. The final same-NAT run hit at
  17.5 seconds; both production runs exceeded Chromium's roughly 15-second initial
  check window. The existing single optional restart then
  selected host/prflx for the same encrypted session. Kernel ARP and global
  neighbor-table pressure make this timing variable; discovery must remain
  within the unchanged 25-second plan lifetime. The measured incomplete-neighbor
  peak was 256 and the advertised ICE socket's send occupancy stayed zero.
  The final same-NAT far-edge run observed 3,058 ARP requests including kernel
  retries, averaging 159/s over its active ARP interval with a maximum aligned
  one-second bin of 256. Full-coverage controls averaged 136–224/s with maximum
  aligned one-second bins of 250–252. The far-edge run's sampled
  global ARP table peaked at 768 entries with `gc_thresh3=1024`, and the kernel's
  table-overflow counter did not increase.
  Retained FAILED entries can pause admission at the global occupancy guard even
  after INCOMPLETE reaches zero; this accounts for the pressure-limited coverage
  run's longer tail. The guard and lifetime are not relaxed to improve a benchmark.
  The #377 Chromium namespace controls separately show that a cold on-link
  browser's request for the bridge can create a STALE neighbor within 26.4 ms
  of the first answer. Cached bridge-MAC and gateway-routed controls do not
  always generate that ARP, so scouting remains necessary. The 20 ms early
  snapshot cadence is based on Linux namespace observations with Chromium; physical
  Android/iOS devices and Wi-Fi access points have not been measured.
  With #377's final source, three independent cold on-link Chromium runs sent
  one early real-port indication each at 44.9–71.3 ms after the first bridge
  answer, received a matching authenticated STUN exchange at 306.8–317.4 ms,
  selected host/host direct with zero restarts, and enqueued zero scouts or
  bridge-originated ARP requests. Three final-source runs each for cached-MAC
  and gateway-routed controls kept the bounded scout fallback and ended on
  direct with one restart recorded. The first browser-nominated direct pair
  can precede that restart; it is not the final application-ready path. One
  additional old-code routed run missed the browser before the 26-second
  observation boundary, so full coverage is not guaranteed in every run; the
  small paired timing samples do not establish latency equivalence. Cold
  different-NAT and missing-srflx controls learned a usable neighbor through
  browser ARP but emitted zero
  feature packets. The 12-case namespace matrix is recorded in
  `web/rtc-lan-upgrade/arp-findings.md`.
- **Paired LAN hint**: `rtc.offer` accepts optional `client_id` under
  `rtc.clientLanCache` (wire 3.12.0). The SPA sends it only after a greeting from
  that paired bridge advertised the capability. It uses `crypto.randomUUID`,
  stores it separately per device and pinned transport key, and removes it on
  unpair or account replacement. Invalid or noncanonical 36-character UUIDs
  are treated as absent. The encrypted session's Opening binds its first valid
  hint, including a post-greeting offer; later offers, ICE restarts and
  `rtc.close` followed by peer recreation cannot change that binding. Carrier
  reattachment retains it; a new session opening may bind a new value. The hint
  is an unguessable client-held bearer value: UUIDv4 has 122
  random bits, is held in that paired browser's storage and sent only inside
  encrypted offers. It is not an authenticated client identity. Anyone who can
  read it already has access to that browser's pairing; a holder can cause one
  extra query to its already-validated address or replace the entry with their
  own freshly validated LAN address. The hint cannot supply an address or skip
  reply validation. A bridge keeps at most 64 previously validated IPv4 LAN
  addresses, one per hint, in a memory-only LRU cache. Entries expire one hour
  after validation; using an entry refreshes eviction recency but not expiry.
  First-wins session binding means each additional cache entry needs another
  session and a validated resolution. For a new name the
  first lookup sends one extra query to that address on its matching interface;
  retries remain multicast only. Every query is QM from port 5353. A fresh
  reply must pass the existing name, arrival-interface/subnet, private-address,
  not-self and DNS TTL checks; the cache never substitutes an answer. Explicit
  source-address probing is Linux-only; other hosts retain multicast discovery.
  Neither the hint nor the cached address enters logs, pushes or diagnostics.
- **Candidate diagnostics**: `rtc.diagnostics` reports remote candidate type
  counts and discovery reasons without names, addresses or credentials. Actual
  direct-check snapshots on ICE restart or close report up to 64 tracked remote
  host endpoints by a bounded ordinal with request/reply counters and successful-pair evidence;
  receiving a host candidate alone means checks pending, never a failed check.
  ICE failure retains a stats-only terminal snapshot until valid restart or
  close, so cleared operational candidates cannot erase earlier checks. A
  peer-reflexive pair contributes evidence only when its remote IP and port
  match an already tracked remote host; relayed pairs never count as direct.
  Success records historical evidence in the generation, not current selection.
  An unresolved sweep-discovered endpoint is reported by sweep and path
  diagnostics without asserting a name-to-address mapping. Reports
  are coalesced by generation/reason; resolution evidence is also delivered when
  needed for an upgrade. Normal logs contain one discovery summary per generation,
  while query detail is debug-level. It uses an authenticated data channel
  (app preferred), and the rendezvous before that. Older clients ignore the new
  push; older bridges
  provide browser-stat-only diagnostics. The greeting advertises
  `rtc.candidateDiagnostics` (wire 3.11.0). The `direct-checks` event and
  pending/not-sent/no-reply/succeeded reasons are added in 3.12.0; old clients
  may ignore those additional diagnostic fields.
- **Initial signaling completion**: channel greeting makes application RPCs
  usable immediately. The rendezvous lease remains until the initial gathering
  completes and every queued candidate RPC has settled, bounded by the offer's
  original negotiation deadline. Recovery follows the same rule. Closing a link
  cancels completion and cannot release a newer restart's signaling lease.
  Local diagnostics separately count gathered, delivered and failed candidates
  per generation. Completed native gathering with no host in its events, local
  SDP or stats reports `browser-no-host-candidates` and keeps TURN instead of
  taking the mDNS upgrade fallback. Path classification uses the transport's
  current `selectedCandidatePairId` before nomination flags, since browsers may
  retain nominated pairs from earlier ICE generations.

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
  new one on probation, and rolls back if it fails. The SPA reads the status
  through the `bridge.update_status` push and the verbs in
  `bridge/src/api/v1/updates.rs`.
- A release build checks once a day. A development build (`provenance.rs`:
  no marker matching the running binary and the service) never checks on its
  own; it checks when asked and nothing in the SPA marks it as having an
  update. If the bridge service runs that exact binary
  (`replaceable_development_binary`), an install with
  `replace_development_build: true` replaces it in place through the same
  verified download and helper, which saves the marker as it was (none is an
  empty backup) and restores it on rollback; the release then carries the
  marker and updates as a release build. A bridge started by hand, or a
  service that runs another executable, is never replaced; the panel shows
  the install script command. The SPA sends the flag only from the warning
  that names the release and the restart. A queued install saves the flag
  with its attempt (`replaces_development_build`); a development build
  stages nothing without it, and at startup drops a schedule it may not run
  and forgets an earlier run's check result and check error, so an older
  SPA has nothing stale to badge. `running_from_cargo_target` (a cargo
  `CACHEDIR.TAG` above the running binary) adds to the warning that the next
  `cargo build` overwrites the release. Controls:
  `planning/v2/Bridge Update Security Checklist.md`.
- `last_error` is a check's or an install's (`last_error_kind`, saved beside
  the status, not sent). A successful check clears a check's; an install's
  lasts until an install succeeds, and only it makes the state `failed`. An
  error saved before kinds were kept counts as an install's only on a
  release build with a saved attempt or a helper result on disk.
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

Git fixtures set a local test identity and `commit.gpgsign=false` through
`git_fixture::configure_repo`, so product commits in the repository and its
linked worktrees cannot use the developer's signing key. Configure clones and
unborn fixtures too. Tests that call product code which creates and commits a
repository in one step use `git_fixture::environment::isolated_git_test!`: it
re-runs that exact test in a child with a cleared environment, temporary HOME
and identity file, and private Git config. Success requires the child's
execution marker as well as its exit status, so selecting zero tests fails.
The fake-signer regression checks
that fixture commits stay unsigned and that the product still honors explicit
signing config. None of this changes Git configuration for `cargo run`.

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
| `spa/src/main.js` | entry: fonts, CSS, theme, the bridge's approve link (`spa/src/core/pairLink.js`, before the router), router, device picker, then `boot()` from `spa/src/views/gate.js` |
| `spa/src/app.js` | re-exports `App`, route handling (`go`, `initRouter`), the `VIEWS` table, `render` |
| `spa/src/appState.js` | shared account and route state (`App`, re-exported by `app.js`); device contexts, feed and cache sync import this without loading the app shell or connection layer |
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

### Files

The Files explorer shares directory upload and New folder actions across
workspace sources, project sources and checkouts. `spa/src/core/filesUploads.js`
mounts them from cached upload capabilities; `fileUploadRpc.js` checks the
current greeting when starting a write. `fileUploads.js` owns an in-memory
queue per device, streams at most two files with sequential acknowledged chunks,
and keeps completed metadata in `ui-uploads` for 30 minutes. Directory drops
create each parent before uploading its files. Successful writes invalidate
their parent listing even after navigation; a mounted tree relists and reveals
the entry rather than depending on a files push, which can omit ignored paths.
`fileUploadTray.js` paints progress, cancellation, retry and explicit replacement
from that queue in a collapsible card inside the preview pane. Account reset
and device retirement cancel and clear the queue; a reload restores only recent
metadata, never file bytes or live upload sessions.

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
- **Drafts and UI state are not replicas** (#206). `spa/src/core/localUiState.js`
  keeps unsent drafts, folds, menus and the other `ui-*` records in their own
  database, `build-ui` (`spa/src/core/localUiStore.js`), versioned apart from
  `build-cache`. A replica schema bump, the lifetime sweeps, `evictEntity`,
  and an older tab deleting `build-cache` on a `VersionError` never reach it;
  only the account reset (`resetApplication`) wipes it. Builds before the
  split kept `ui-*` records in `build-cache`: they are carried across,
  newer-only, at a page's first UI-store use (mount reads wait for that pass,
  bounded) and whenever another tab announces a `ui-*` write on the replica
  channel, then removed from `build-cache` if unchanged. The replica upgrade
  keeps `ui-*` records until they are carried. Both databases share the
  connection handling in `spa/src/core/idbDatabase.js` and the key, stamp and
  announcement helpers in `spa/src/core/idbRecords.js`.
- **Draft ownership cleanup** (#209). `spa/src/core/uiDraftLifetime.js`
  runs after an ordinary sync pass or push delivery finishes writing,
  detached from the path that paints the cache. Each list's write transaction
  captures the ownership it replaces; cleanup then captures draft writes.
  A possible deletion triggers fresh workspace, project and board reads
  before any draft can go. Their complete
  conversation rosters and positive ownership evidence retire drafts for
  deleted owners. Workspace settings and directory addresses identify their
  workspace directly; other
  workspace drafts use previously cached ownership. A present conversation
  anywhere wins, including on unwatched runs. Unknown, provisional and legacy
  owners remain when the lists cannot establish deletion; no age cap touches
  a live owner's draft. Cleanup does not prune project-keyed task composers
  (`task-composer:`), provisional chats (`chat:draft:*`), project settings
  (`project-settings:`), task comments (`tracker-task:*`), bare-project Git
  and changes drafts, or global `compose:` and `new-project:` drafts. Those
  need their own ownership rules.
  Pushed owner lists start cleanup only when they remove a previously cached
  owner ID; unchanged membership, including git-flush lists, adds no reads.
  Ordinary sync confirms only when its lists suggest a captured draft is
  obsolete; a push confirms only when recognized drafts exist. A list may
  predate a newly created owner, so its own response cannot authorize cleanup
  of drafts captured afterwards. Neither UI-store recovery nor a
  cleanup rejection delays ordinary list writes or later pushed items.
  Failed/incomplete reads, intervening ownership writes
  and stopped sync leave drafts alone. Conditional deletion checks each
  captured write inside the UI-store transaction, so another tab's newer
  edit survives even at the same timestamp. Replica eviction still never
  reaches drafts.
- **A lost connection is weather, not a verdict** (#169). iOS drops a suspended
  page's IndexedDB connection, and the first opens after a resume fail with
  `UnknownError: Connection to Indexed Database server lost`. The cache reopens
  on a backoff (`reopenDelaysMs`, about eight seconds) whose retries wait for
  the page to be shown, then rests until the next wake (page shown, `pageshow`,
  `online`, or 10 s) and tries again. The backoff lives in the one shared
  reopen, not in each operation, so writes retried through it keep the order
  they were asked in. Each open is given five seconds, which bounds one attempt
  but cannot rescue a hung one: later opens queue behind it. A read is never
  answered "nothing" because the database was away: it waits until a read
  really answers, so a surface keeps what it painted and a merge never takes
  "unreadable" for "empty". A write waits the same way, and one that keeps
  failing while the database answers fails alone, as does one that does not fit
  a full quota. Only an error no reopen can fix (a private window refusing
  IndexedDB, a schema error) stands the cache down for the session — and so
  does a failure that outlasts the weather: a round of failed opens before the
  database has ever opened in this page (a store Safari cannot open, with the
  boot paint waiting on it), a minute of failing while the page is shown, or
  ten seconds of an upgrade blocked by a tab on an older build. A hidden page
  waits on. `cacheHealth()` answers the state now (Settings shows it as the
  "Local cache" line, the console as `buildCacheHealth()`), and every loss,
  rest, recovery and stand-down is recorded in the connection diagnostics under
  `local-cache`.
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
  Task body checklist changes (#347, `spa/src/core/taskChecklist.js`) rewrite
  only the checked character of a marker identified by the markdown renderer,
  then save through `tasks.update` with the original body's SHA-256 digest as
  `expected_body_hash`. The bridge checks the exact saved bytes under its app
  lock and refuses a changed body with `stale_body`, preserving edits from
  other devices and agents. The greeting's `tasks.bodyPrecondition` is cached
  in `taskChecklistSupport.js`; body boxes enable only when that cached fact
  says the bridge supports safe ticks. The device caller gates dispatch on the
  current greeting too. Saves run one
  at a time; a per-task Web Lock orders saves and the brief pre-read cache
  snapshot across tabs, with a shared queue for mounts in the same tab when
  Locks are unavailable. A refusal conditionally restores that body while retaining newer
  fields and timeline entries. Reads begun before or during a save cannot undo
  its optimistic copy, with authority and the pre-read body checked inside the cache transaction;
  a read after the save or refusal confirms the current server body. A refusal
  never retries the stale body. Checkboxes elsewhere are disabled.
- **A reconnect keeps the route** (#170). The gate (`spa/src/views/gate.js`)
  hands the app back through `renderUnlessStanding()` in `spa/src/app.js`,
  which leaves the page in `#root` as it is (nodes, scroll, focus) when it was
  built for this route over the device context that is still the registry's.
  The sessions and subscriptions re-attach under it, and it repaints when a
  cache write announces. It is built again only when nothing stands there:
  a gate screen took `#root` (the version gates, the waiting and onboarding
  screens unmount the view as they take it), the account changed, or the
  route's machine was retired since (`deviceContextIdentity`).
- **Route surfaces** (branch, task, tracker task, project, workspace) and the
  shell's rail and console stand on `surfaceContext(route)`
  (`spa/src/core/surfaceContext.js`): the device's context, or a session-less
  one when only its records are on disk (a cold reload). They never ask whether
  the machine can answer. `mountDeviceNotice` (`spa/src/core/deviceNotice.js`)
  stands in only for a machine nothing here has ever held.
- Entity-specific caches sit beside it: `taskCache.js`, `trackerCache.js`,
  `conversationCache.js`, `surfacesCache.js` in `spa/src/core/`.
- **Bodies in pages** (#95). A file, diff or commit patch over its cache cap
  (`FILE_MAX_BYTES`, `FILE_DIFF_MAX_BYTES`, `CHANGESET_DIFF_MAX_BYTES`,
  `COMMIT_PATCH_MAX_BYTES`, `WORKING_DIFF_MAX_BYTES`, `ATTACHMENT_BODY_MAX_BYTES`)
  is never painted from the answer. Its usual record becomes a head (the
  body's metadata, `paged: true`, no text) and the text is kept as page
  records (`spa/src/core/bodyPages.js`, kind `page`): one per page, chained by
  byte offsets from 0, each naming the body it was cut from (the bridge's
  `range.version`: a file's version, a digest of the whole patch; or
  `whole` for an answer split here) so two versions are never joined, and a
  page of another version reads the body again from the top. From a bridge
  announcing `bodies.pages` the pages are read with `range` as the reader
  reaches the end of what is painted (`createPagedBody`, the `pages` option
  of `createCachedBodies`, `spa/src/core/pagedFileView.js` for the Files tab,
  the diff viewport's `onNeedMore`): a commit's by the rows of the file its
  pages end inside, and a line longer than a page as the reader goes along
  it, painted as far as it has come; media is read whole into
  pages, since a picture cannot be shown half loaded. An older bridge's whole
  or cut answer is split into pages here and painted the same way, with the
  truncation notice where it was cut. A page read is a wire call: what it
  brings is kept only while the head is still the write it started from, and
  pages joined under a head are read on from only under that same write —
  both told by the record's write name (`recordWriteOf`, `record.write`),
  never its `at`, which two writes on one millisecond share. Pages live and
  die with their head: the five recent files per workspace, the workspace's
  data TTL. An oversized aggregate diff is kept without its patch and
  painted file by file.
- **Paged task lists.** From a bridge announcing `tasks.listPaged`, the sync
  pass and the Tasks tab pull `tasks.list` a page at a time
  (`spa/src/core/trackerPages.js`): each page read is written under its own
  address (its filter, cursor, limit and read number) with that read number
  in its body. The page is read back and laid over the list using its own read
  number and cursor. A completed walk removes only older pages of its filter,
  leaving newer reads alone even when they are still awaiting readback. Each
  page updates the list record for exactly the numbers it answers for, so the
  tab fills in page by page. A page can be short or empty and still name the
  next (under a label or assignee the bridge reads a bounded stretch per page);
  the pull walks on until no next is named. Every read takes a number, as it
  is asked, from a count shared by all tabs in the cache. Allocation waits
  through transient cache recovery; a refused counter write stops the read
  instead of inventing a tab-local number while shared storage is usable.
  Each page notes the stretch of task numbers it had the say on beside the
  list record, written in the same transaction as that list record
  (`spa/src/core/taskReadOrder.js`,
  `mergeCachedTogether`). That joint write waits through transient cache
  recovery before the walk advances; a refused fold stops the walk without
  advancing its cursor. A page yields every row a read asked after it had
  the say on, in any tab, present or absent, so an older page neither brings
  back a task a newer read took off the list nor overwrites a newer copy
  with the same `updated_at`. A page answers for its own rows
  and below with its own read; the numbers between the last row the walk laid
  and a page's first keep the older read of the pages that read past them,
  since the cursor does not say how far the page before read. For writers
  that are not page reads (a whole list from an older bridge, a task filed
  here) the timestamps stand in: a held row written after the page's copy of
  it, or one the page does not name that was written after the page was
  read, keeps its place. An older bridge is read whole, as before.

No timer polls the bridge for data. The data timers are the device presence
poll against skriftapp and the served-version check. The presence poll runs
every 15 s in the app and every 3 s on the waiting screen, and every second
after this page approves a device, until the cached list calls it online or
90 s pass (`spa/src/core/pendingPairing.js`, #321). The transport has its own:
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
  - the agent rail reads `settings.get` (`spa/src/core/agentRail.js`);
  - workspace sources and project directories paint their held Git records,
    then check `git.status` with `if_status_key` on mount. Workspace review
    also checks `git.unpushed` with `if_diff_key`: a published base can change
    without moving the status key. An unchanged warm workspace remount makes
    at most these two requests, without reading refs, history or diff bodies.
    Changed answers invalidate only that directory's dependent records.
    Board notifications trigger the same checks for visible panes; hidden
    panes defer them, and hidden ref pickers defer reads until shown. Explicit
    refresh still reads through. Ref checkout drops mutable Git records before
    remounting; immutable commit patches stay cached.
- **Invalidation pushes.** Some push fields only say what moved, and the
  applier reads again:
  - `tasks` carries only ids, so the project's task list is re-read;
  - changed `files` paths re-list the directories the reader opened and
    re-read open file bodies with `fs.read` (a paged body from its first
    page, keeping the rest when the file's version has not moved);
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
  `term` channels and times out after 15 s. A relayed link settles for 20 s,
  then samples direct viability with a 5/10/20/40/60-second backoff, remaining
  at one minute while waiting for evidence. It permits one optional ICE restart
  for the peer link's entire lifetime when an alternate direct pair succeeds or
  a browser mDNS name resolves for the current generation. Eligible unresolved
  host-sweep evidence can also spend that existing budget when fresh browser
  checks are needed; skipped sweeps and absent browser host candidates cannot.
  Recovery restarts do not renew the optional budget. Failed or ping-exhausted
  browser pairs are not revived: every new generation starts the early sweep
  against its current-generation candidate port. Chromium namespace measurements observed
  unreplied host checks for about 15 seconds; Safari timing is inferred from
  [WebKit/libwebrtc source](https://github.com/WebKit/WebKit/blob/main/Source/ThirdParty/libwebrtc/Source/webrtc/p2p/base/basic_ice_controller.cc#L271),
  not measured on iOS. That restart holds relay candidates
  behind host candidates again and retains its rendezvous lease through gathering
  and delayed candidate signaling. A gathering timeout on a still-carried path
  reports pending because native ICE may still nominate later. After the attempt
  finishes, path observations stop within 120 s; this allows slow nomination
  without continuing stats work for the session's lifetime. Moving traffic to a
  direct pair does not close the browser's TURN allocation. Retiring that
  allocation is a follow-up: browser capabilities and ICE reconfiguration risks
  need investigation. `spa/src/core/transportPath.js` classifies the carrying path.
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
  `api_version` against `SPA_API_RANGE` (`>=2.0.0 <4.0.0`). A greeting with
  no version, or no greeting at all, reads as `PRE_ALPHA_API_VERSION`
  (`0.0.0`). For compatibility, that is matched at the lowest adapter's floor
  rather than rejected. Any other version outside the range returns
  `{unsupported: "bridge" | "app"}`, and `spa/src/views/versionGate.js` says
  which side needs updating.
- `capabilitiesOf()` in `spa/src/core/bridgeApi/v1/index.js` turns the greeting's
  `capabilities` array into feature flags. Every bridge the adapter admits sends
  one; the minor-version table that stood in for it went with 1.x. New features
  get a name.
- Surfaces read the flags with `bridgeCapabilities(deviceId)`
  (`spa/src/core/changeEvents.js`), which falls back to `NO_CAPABILITIES`.
- `conversation.reset` also gates the generation fields added to existing
  conversation requests in wire 3.11. The adapter restores their old shapes
  with `spa/src/core/bridgeApi/v1/threadParams.js` when the greeting lacks the
  capability, retaining established selectors and paging parameters. Sessions
  read their device's cached reset capability from `conversationResetSupport.js`
  before hello: a cleared conversation keeps its required generation fields on
  reconnect, and a device without cached support uses legacy shapes. A greeting
  that settles during that read takes precedence at dispatch. Revision reads
  retain `agent_id` and `conversation_id`, which older bridges already accept.
- The task and conversation watch switches, task attachment buttons, and
  conversation compaction settings are available before a greeting (#182).
  Their values come from cached task and agent records. A late greeting does
  not replace the controls or their drafts, and an older bridge refusing a
  chosen command is explained in plain language (`commandRefusal.js`).
  Automatic task read marks still require watch support; a mounted task page
  resumes that housekeeping when its device greets.
- A flag that changes what a view draws is written to the cache at the
  greeting and read from there, so a cold mount draws what it will keep:
  `tasks.commentUserNotifies` becomes the per-device Needs you rule in
  `spa/src/core/needsYouRule.js`, read by the Tasks tab and the inbox's
  watched tasks; `branches.finishDelete` becomes whether Done deletes the
  branch on that machine in `spa/src/core/branchDeleteSupport.js`, read by the
  branch surface's Done and the inbox row's; `agents.createdBy` becomes
  whether that machine's agents name their makers in
  `spa/src/core/agentLineageSupport.js`, read by the lineage reader with the
  rows, so the Agents panel lists Build agents only for a bridge that
  announced them; `fs.projectSources` is remembered in
  `spa/src/core/projectFilesSupport.js` for the project's Files notice.
  Project-source requests check the current greeting at dispatch through
  `spa/src/core/projectFilesRpc.js`. What the cache says draws the
  confirmation; the deletion itself is sent through `whenGreeted`, on the
  adapter the current session's greeting installed.
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
- A machine's model catalog (`spa/src/core/modelCatalog.js`) is read once and
  kept. A `models.changed` push from its bridge (`DEVICE_EVENTS` in
  `spa/src/core/changeEvents.js`, acted on armed or not, like
  `bridge.update_status`) asks it again if any surface has wanted it, and the
  answer lands in the cache like any read. The model pickers offer what that
  machine's CLI runs and draw `modelNoteHtml` (`spa/src/core/modelPicker.js`)
  under the model select: "Update Claude Code to 2.1.284+ for Claude Sonnet
  5.5." A saved model the CLI is too old for stays selected, named with the
  version it needs. A bridge before 2.2.0 sends neither, and nothing is drawn.

### Surfaces

Routing is hash-based: `spa/src/core/router.js` parses and builds routes, and
`VIEWS` in `spa/src/app.js` maps a route kind to its renderer. Settings routes
open as a modal (`spa/src/views/settingsModal.js`).

| Surface | Files |
| --- | --- |
| Inbox | `spa/src/views/inbox.js`, `spa/src/core/inboxShell.js`, `spa/src/core/inboxView.js`, `spa/src/core/inbox.js` |
| Conversation and agent rail | `spa/src/core/shell.js`, `spa/src/core/agentRail.js` (+ `agentRailModel.js`, `agentRailRender.js`), `spa/src/core/chatRepository.js`, `spa/src/core/thread*.js` |
| Tasks list, board, dashboard | `spa/src/views/projectView.js` → `spa/src/core/trackerTasksPane.js`; `trackerListRender.js`, `trackerBoardRender.js`, `trackerDashboardRender.js` in `spa/src/core/` |
| Task page | `spa/src/views/trackerTaskView.js` → `spa/src/core/trackerTaskPage.js` |
| Workspace navigation | the toolbar's `project / workspace` picker in `spa/src/core/toolbar.js` (+ `toolbarModel.js`, `toolbarRender.js`); the rail down the left edge (Changes, Files, Tasks, Settings) in `spa/src/core/workspaceRail.js` over `directoryRail.js`, mounted from `spa/src/views/workspaceView.js`; inside Changes, the per-directory tab row in `spa/src/views/workspaceChanges.js` |
| Project navigation | the rail down the left edge (Tasks, Workspaces, Settings) in `spa/src/core/projectRail.js` over `directoryRail.js`, mounted from `spa/src/views/projectView.js` and `spa/src/views/trackerTaskView.js` |
| Changes and git | `spa/src/core/gitPane.js`, `gitRender.js`, `changesReview.js`, `changesModel.js`, `changesRender.js`, mounted from `spa/src/views/workspaceChanges.js` (a workspace, under its directory tab row, one pane per directory shown, kept while another stands; hide/show suspends owned editors, dialogs, listeners and layout work while preserving drafts, scroll and cache subscriptions) and `spa/src/views/branchView.js` |
| Files | `spa/src/views/files.js`, `spa/src/core/fileRoots.js` (a workspace's one root per directory), `spa/src/core/fileTree.js` (+ `fileTreeModel.js`), `spa/src/core/fileTabs.js` (+ `fileTabsModel.js`), `spa/src/core/fileViewer.js`, `spa/src/core/fileEditor.js` |
| Terminal | `spa/src/core/console.js`, `spa/src/terminal/` |
| Settings | `spa/src/views/settingsModal.js`, `settings.js`, `deviceSettings.js`, `devicePanels.js` in `spa/src/views/`; `spa/src/sheets/` |

Inbox rows, including Recent and the project Workspaces tab, use
`spa/src/core/inboxStatusDot.js` for one right-edge status dot. Unread activity
is green; running activity without unread is grey. Only running agents pulse,
and reduced-motion preferences disable the animation. Watched tasks read their
assigned agent's running state from cached rosters. A folded project head
summarizes watched activity, including tasks without a Needs you row; an
expanded head uses only its project conversation's own unread and activity.
The head offers Hide and New workspace directly.

The inbox and agent rail share `agentIsRunning` in
`spa/src/core/agentRunning.js`: an agent runs while its own loop or an agent
in its activity panel runs. The task feed derives those descendant rollups
from the freshest cached rosters, per device and project, using the cached
`agents.createdBy` capability for Build descendants. The agent's own
`working` flag stays separate for interruption controls.

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
| Bump the wire version | `API_VERSION` in `bridge/src/api/mod.rs`, `"current"` in `fixtures/api/versions.json`, `fixtures/api/v1/session.hello.json`, and the new fixtures' `since`. A named feature goes in `FEATURE_CAPABILITIES` (same file). Then `node scripts/api-verbs-manifest.mjs` writes `fixtures/api/verbs-<previous minor>.json` (delete the old one; for a new major, pass the commit the previous release shipped from, and for a later patch of that major's first minor, the commit its `.0` shipped from): the contract tests hold every verb and capability absent from it to the new `since`, and only a new major's `.0.0` may drop one |
| Add a view | a route in `spa/src/core/router.js`; `spa/src/views/<name>.js`; register it in `VIEWS` in `spa/src/app.js`; rail or console via `spa/src/core/shell.js`; styles in `spa/src/styles/`; tests in `spa/test/` |
| Change the Files explorer | `spa/src/views/files.js` shares previews, edits and file tabs; `spa/src/core/filesRoots.js` provides project/workspace roots; `fileRoots.js` and `fileTree.js` mount their trees; `directoryScope.js` separates directory records from layout state |
| Add an MCP tool | a `BridgeAction` variant with its `tool_name()` and `surfaces()` arms and a schema in the right `*_tools()` list in `bridge/src/mcp.rs`; handle it in `bridge/src/app/mcp.rs` (or `bridge/src/app/conversations/` for thread actions) |
| Add a harness | a `Harness` impl in `bridge/src/harness/`, an `AgentProvider` variant in `bridge/src/models.rs`, and its arm in `harness_for()` |
| Change what an agent runs under | `bridge/src/priority.rs`, `bridge/src/service/systemd.rs` |
| Render markdown, or add a reference shape | always `markdownHtml(text, { place, mode })` from `spa/src/core/markdown.js`, the one entry point (`spa/test/markdownEntry.test.js` fails anything else); blocks in `spa/src/core/markdownBlocks.js`; reference syntax in `spa/src/core/markdownRefs.js`, links and labels in `spa/src/core/markdownLinks.js`, web links (`[text](url)`, bare http(s) URLs; http/https/mailto only, new tab) in `spa/src/core/markdownWebLinks.js`, resolution against the account-wide index in `spa/src/core/referenceTargets.js`/`referenceIndex.js` (filled by `referenceIndexFeed.js`); teach agents a new shape in `bridge/templates/notes/link_markup.md`, the protocol bullet in `bridge/src/orchestrator/workspace.rs` and `reference_shapes_note!` in `bridge/src/mcp.rs`; the XSS rules are `planning/v2/Markdown Rendering Security Checklist.md` |
| Change how a device is paired | the bridge's printed block and approve link in `bridge/src/pairing.rs`; the installers' framing in `scripts/install.sh`; `#/pair/<code>` in `spa/src/core/pairLink.js` opens `spa/src/sheets/addDevice.js` (`openPairingLink` in `spa/src/views/gate.js`); the fragment survives sign-in through `withArrivalFragment` in `skriftapp/buildapp/landing/passkey-signin.js`; the rules are `planning/v2/Device Pairing Security Checklist.md` |
| Change a colour | tokens in `spa/src/styles.css` (both themes) |
| Change transport or ICE | `bridge/src/rtc.rs`, `bridge/src/rtc/policy.rs`, `spa/src/core/peerLink.js`, `spa/src/connection.js` |
