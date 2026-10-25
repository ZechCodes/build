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
| E2EE protocol | `transport/js/` (SPA/web binding), `transport/python/` (reference/tests); Rust in `bridge/src/transport.rs`. Compatibility tests and fixtures live in this checkout | both ends; Python is test-only |

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

`config.rs::resolve` selects `BRIDGE_CHANNEL` (`prod` when absent; `dev`;
anything else errors before command dispatch). Dev defaults API/web to
`https://dev.getbuild.ing`, relay to `wss://relay-dev.getbuild.ing`, and all
bridge state to `~/.local/state/build-dev/`: identity, config, tasks/SQLite and
documents, projects, worktrees, `mcp.sock`, and logs. Individual `BRIDGE_*`
overrides still win, but dev refuses any resolved state path or environment
project path at or inside the production state root (`~/.build`). The check
resolves symlinks and missing leaf components without creating files; inability
to resolve a path is an error. This includes the state and log roots, identity,
config, tasks, the task-derived harness root, projects, worktrees and MCP socket.
Dev MCP and project registration consume the validated paths, including home
expansion. Backup and service installation use the same resolved paths as serve.
Service environments pin the channel, URLs and resolved paths;
MCP launches carry the channel and socket, while private-key stripping remains.
Dev ignores environment-provisioned device keys and pairs its own identity.

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

**Task assignment** records durable authority in schema-17 delivery intents
before provisioning or admitting a message. The saved target, actor, note,
operation identity and generation survive restart. Waiting assignments create
no worker or workspace. `app/tracker/delivery_service.rs` runs from shared
startup without a client, claims bounded work, runs checkout work off the app
lock, and revalidates scope and generation before admission. Explicit immediate
assignments use the same delivery steps. Frozen message parameters and origin
land before receipt admission, whose transaction checks current graph and
authority. Recovery settles an existing receipt without rebuilding its message.
The durable notice outbox tells the coordinator about unassigned releases and
holders about reblocking. Assignment and coordinator briefs list every retained
prerequisite closed without Done and direct assessment and user escalation.
`completion_authority.rs` binds Complete to the assignment operation actually
consumed through Sent/Seen; a queued replacement cannot authorize an older
turn's report. The RPC and push layer remains separate from this service.
Background provisioning owns the ordinary filesystem-job count until settlement,
so lock, rename and reclaim refuse while it waits for source synchronization.
Reserved deferred workspaces prepare their unfinished saved sources through the
same manifest retry path and canonical SyncLock as workspace retry; changed
project settings cannot redirect the saved cut or leave Git materialization
without a pinned commit. Worker source-location holds remain independent.

**Frames.** A request is `{id, method, params}`, with optional typed `write`
metadata (`operation_id`, optional `after`) for journaled commands; a reply is
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

The nine SQL-lane methods (`thread.page`, `thread.activity`, `thread.operation`,
`thread.revision`, `tasks.list`, `tasks.get`, `tasks.waiting_on.blocks`,
`writes.status` and `writes.subscribe`) enter a separate lane
before terminal or write admission. `carrier/sql.rs` owns eight dedicated
threads and a fair queue with at most 16 waiting callers per session opening.
Callers waiting for a read connection still count against that bound. Excess
SQL parks ordered intake on its carrier; it never returns capacity `busy`.
Queued compatible callers may fold, but a new caller never joins a running
query. Carrier release prunes its queued callers and fences running replies;
a replacement carrier keeps its opening usable. Close signaling wakes a
parked intake independently of the channel event pump. Stats and the existing
ping/signaling fast paths bypass this lane. The daemon calls
`FrameHandler::start_read_workers` after creating its shared handler and before
constructing intake or accepting relay/client work, so all eight SQL workers
are ready before the first read. Handler clones share that executor.

The fifteen published reads (`board.list`, `project.list`, `workspace.list`,
`workspace.get`, `agent.list`, `settings.get`, `models.list`, `harnesses.list`,
`bridge.update_status`, `capture.get`, `task.get`, `task.stages`, `tasks.columns`,
`stream.events` and `stream.state`) answer at intake from immutable snapshots.
They take no SQL permit, dispatch slot or app lock, including when those lanes are
full. Typed parameter normalization and result validation still apply to v1
verbs; the two stream reads retain their legacy shapes and QA-only gate.

The task detail and stage snapshots follow committed owner publication,
including changes to a linked implementation. Task history is rendered from
the same published conversation history as workspace detail; unwindowed task
reads retain the resident-history boundary. Maintenance captures plan IDs under
the app lock, probes canonical document directories in its existing unlocked
phase, and commits versioned availability observations. Successful ingestion
publishes availability directly, fencing older observations. Capture writes
publish their saved record, and retirement removes it. QA stream producers
publish each event and its updated head together; an incremental output digest
keeps state reads independent of log length. The SPA still calls `task.get`
and `term.list`, so both surfaces remain available. `task.get` is a registered
v1 planning read; `term.list` retains its legacy wire contract.

Terminal listing's membership snapshot publishes independently of temporarily
borrowed plan/run owners. Listing reads each terminal's coherent atomic size
pair, updated after resize, so it does not wait on screen parsing or PTY output.

The 23 observation reads (filesystem/Git, stage documents, review
reads, attachments, archives, GitHub repositories, Git initialization options
and `term.list`) enter an independent fair I/O lane before write admission.
It reuses the SQL lane's bounded per-opening queue and queued-only folding, with its own
16-caller backlog and up to eight dedicated I/O threads. I/O threads start on
demand and are reused; they are not all started before the first read. Excess
intake waits without capacity refusals. Review and task-attachment metadata preparation runs on the
same fixed SQL executor as ordinary SQL reads; its transaction and connection
are released before any filesystem/Git work. Pure I/O reads take no SQL slot or
connection. Carrier closure cancels preparation waits and suppresses replies
from released carriers; compatible callers on surviving carriers remain live.

Mixed jobs remain counted in the same 16-caller opening backlog while pending,
preparing metadata or ready for I/O. Separate asynchronous metadata coordinators
wait for the shared SQL executor and connection pool; physical I/O coordinators
select only pure jobs or fully prepared jobs. Already admitted pure jobs can
advance within that opening or on other openings while metadata waits for SQL.
A full opening still parks its subsequent ordered intake. Queued-only folding,
carrier cancellation and the eight-worker physical-I/O bound remain in force.

Other queued work uses the worker pool in
`bridge/src/carrier/dispatch.rs`, which runs handlers on blocking threads.
Before that pool, a bounded per-wire admitter in `carrier.rs` feeds frames in
arrival order; its overflow answers retryable `busy`. The pool has eight
workers and separate foreground/background queues, with at most two workers
running background frames. Terminal lanes and immediate handlers run outside
that eight-worker bound. Deferred writers release the app lock during external
work and retain their dispatch worker until they finish.

**Dispatch.** Each handled frame goes to `dispatch_frame` in
`bridge/src/app/rpc.rs`, which tries these in order:

1. `signaling()`: `rtc.offer`, `rtc.ice`, `rtc.close`, answered without the app
   lock (`bridge/src/app/rtc.rs`).
2. the update-admission gate (`bridge/src/update/admission.rs`).
3. `session_scoped()`: verbs that need the caller's own `SessionSender` or the
   shared state (`session.hello`, `bridge.stats`, `term.*`, `agent.attach`,
   `agent.start`, `agent.interrupt`).
4. `routed()` → `AppState::route` → `crate::api::v1::dispatch`, then the legacy
   arms (`ping`, `term.list`, `term.close`, and the QA stream fixtures). Native read frames
   have already entered their snapshot, SQL or observation lane at intake.

Routed requests retain their update-admission lease through the reply and
share it with delivery and compaction workers. Their drain does not acquire
another gate handle or lease. A request admitted without an update service
is distinct from an exempt liveness request, whose drain still checks the
gate independently. Project-deletion refusal runs inside the routed dispatch
guard before deferred work can be taken. Session-scoped and standalone drains
keep their existing admission checks.

Idle update handoff reserves a pending claim under the admission mutex, then
runs its busy probe without that mutex. The claim blocks new non-read work;
read lanes can still acquire counted leases. Handoff closes admission only
after a quiet probe and a brief recheck that no leases remain. Rollback and
reopen invalidate the claim before notifying waiting work.

**The verb registry** is `bridge/src/api/v1/mod.rs`. Each family file (`board.rs`,
`changes.rs`, `git.rs`, `harnesses.rs`, `push.rs`, `tasks.rs`, `lifecycle.rs`, `thread.rs`, `updates.rs`,
`workspace.rs`) has a `methods()` table built with the `v1_method!` macro, which
names the verb, its handler and its typed params and result. Handler signatures
never take `serde_json::Value`; a test in that module enforces it. Slow git work
is deferred and runs with the lock released.

The scoped file mutations in `api/v1/git.rs` are individually announced
capabilities, added in wire 3.13.0 (#391) and 3.16.0 (`fs.createFile`, #433):

| Verb | Params beyond scope | Result |
| --- | --- | --- |
| `fs.createDirectory` | `parent`, `name` | `path` |
| `fs.createFile` | `parent`, `name` | `path` |
| `fs.uploadBegin` | `parent`, `name`, `size`, optional `replace` | `upload_id`, `path`, `chunk_bytes` |
| `fs.uploadChunk` | `upload_id`, `offset`, `content_b64` (no scope) | `received` |
| `fs.uploadFinish` | `upload_id` (no scope) | `path`, `size` |
| `fs.uploadAbort` | `upload_id` (no scope) | `{}` |

Creation and upload begin resolve the same server-owned source/worktree ids
as `fs.tree`. Their `parent` is scope-relative (empty means the root), and
`name` is one child name. Result paths are scope-relative too. Uploads accept
binary files in chunks of at most 4 MiB decoded bytes, staged until finish;
the initiating session owns the upload id. The default refuses an existing
destination with `already_exists`; `replace: true` explicitly permits file
replacement. The host-directory browser's `fs.mkdir` remains a separate verb.
Uploads are bounded at 256 MiB, eight open uploads per client session and 64
in total; a full upload pool returns `busy`. The initiating session opening
owns the ID. Its end or ten minutes without a chunk removes staging, and Abort
can cancel an owned upload while reclaim reserves its workspace. Scope roots
are canonicalised before the no-follow parent walk. Before admitting clients
on boot, the bridge sweeps its known scope roots for orphaned regular files in
the reserved `.build-upload-<UUIDv4>.part` namespace, owned by its user with
mode 0600. It skips active IDs, symlinks and `.git`, with bounded traversal.
Creation refuses that reserved staging destination namespace, including case
variants, so a published user file cannot be mistaken for an orphan on boot.

File writes, creation, host `fs.mkdir`, and upload operations use the deferred
drain in `app/fs/deferred.rs`. They capture source identities under the app
mutex, resolve paths and acquire filesystem-root serialization off it, then
validate the captured records before running filesystem service work. Final publication
invalidates observations and installs any reopened workspace record; its
manifest is persisted off the app mutex. The existing deferred-job count keeps
workspace removal and metadata changes out while these stages run.
Filesystem-root locks have their own namespace keyed by the canonical root,
separate from Git's common-directory `source_sync::SyncLock`, so a fetch or a
Git mutation does not make an editor save wait for the repository lock.
Editor writes and upload finish also hold a destination lock keyed by the
canonical parent and literal filename, so overlapping source roots serialize
revision checks and replacement. Their contention waits share a five-second
budget, and both remain held through final publication.
An in-memory lease also retains the recorded source root through that final
publication. `project.remove_source` refuses `busy` while that source is in use;
unrelated sources can still be removed. The check does not resolve paths or
wait for filesystem work under the app mutex.

Filesystem service and final publication are separate stages. A write can
already have changed bytes on disk when publication refuses a moved source
or a new reclaim reservation, or when persisting the reopened workspace fails.
The error response does not roll back those bytes. Closing this window belongs
to step 6b's acknowledgement and confirmation work; the current staging does
not make disk changes and publication one atomic operation.

Upload admission and quota accounting remain in app state. Staging descriptors
have separate ownership, so chunk writes, publication, cancellation cleanup,
session-end cleanup and idle cleanup run outside the app mutex. Finish claims
publication before its sync and rename: cancellation before that claim wins;
after it, finish owns the result and invalidation even if the session closes.
Cancellation does not wait for a staging mutex held by an in-flight service.
That service retains the staging file and performs cleanup after it finishes.
Intermediate stages retain the verb's original result-type check. Only the
final successful stage stamps user activity; that existing `save_user_session`
call still runs under the app mutex and remains a known #681 violation for step 6b.

`thread.page` is a message-counted page with folded activity digests. Wire
3.20.0 announces `threads.leanActivity`: with optional `activity: "newest"`,
each activity run between non-activity items ships exactly its newest item,
including a live tail run. Messages and other non-activity items keep their
existing paging behavior. `activity_digests` retain the exact full-run span,
row count, tool-call count and newest tool call even when most rows are
omitted. The bounded store reader reads only the activity items it ships.
The mode works with `before_sequence`, `after_sequence`, and
`after_sequence` plus `newest: true`. Omitting `activity` or sending
`activity: "bounded"` keeps the existing per-run cap and page activity budget.
A client sends the new param only when the greeting names the capability;
`thread.activity` fetches the digest's span when the client opens that run.

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
same subscriptions session: the legacy invalidation pushes went in 3.0.0. A
subscription's scope is `board`, `entity`, `all` or, since 3.21.0,
`project_source` (one original project source; see Projects and sources). The
events the bridge announces are `ANNOUNCED_EVENTS` in the same file
(`changes`, `bridge.update_status`, `models.changed`, `rtc.diagnostics`,
`harnesses.changed`, `compaction.changed`). Terminal output
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
and `store/writer.rs::StoreWriter` wraps the sole writable connection. Store
clones and journal admission share that connection. The nine SQL-lane RPCs instead borrow
one of eight read-only, query-only WAL connections from `store/read_pool.rs`.
A fair asynchronous permit supplies one borrower; a fresh request transaction
covers all related rows and ends before the connection returns. Those RPCs
never acquire the app mutex or use the writer's connection. Remaining Store
callers still serialize on the writer connection. Routed v1 handlers receive
`&mut AppState`, and their Store calls can execute while the app mutex is held.
`off_the_workers` protects runtime workers from blocking; it does not remove
these serialization points.

Before publishing its eight readers, `ReadPool::open` initializes each SQLite
schema by preparing and dropping `SELECT 1 FROM main.sqlite_schema LIMIT 0`.
This is startup work, without an application-row/result cache or a retained
statement. The pool also retains the private writer connection until its last
reader closes, including cloned pools and outstanding borrowers. Closing the
readers before the final writer permits SQLite's final WAL checkpoint and
preserves database-file-only backups. Read paths do not acquire the retained
writer's mutex.

Task identity/done-at backfills and cursor key creation happen before reader
admission; task reads retain pure derived fallbacks for older records.
Parked-project revival, legacy workspace adoption, manifest refresh, board
attention and cache/sweep work run during initial warming and from lifecycle
or periodic producers. Published list/detail reads do none of that work.

`timing.rs::FrameClock` exposes per-method `queued`, `lock_wait` and `held`
aggregates through `bridge.stats`: each has `count`, `sum_ms`, `p50_ms`,
`p95_ms` and `max_ms`. Counts are completed computations (one for a folded
read), including zero wait/hold when no app lock was taken. Wait and hold sum
all app-lock acquisitions within a computation. Queue time starts at carrier
intake and includes per-wire admission, dispatch and blocking-pool scheduling;
a fold keeps the earliest caller intake, even when carriers reach dispatch out
of order. Immediate signaling/attachment handlers include blocking-pool
scheduling too. Intake-only pings/refusals and
work dropped before a handler starts remain outside the served counts.
Registered v1/legacy/QA methods and fixed internal work labels have bounded
records; unknown names share `other`. Histograms use fixed buckets, so
percentiles are estimates; sums/max retain microsecond resolution. Existing
total/hold keys and slow-frame log formatting remain. Per-acquisition
`lock_acquisition_wait` and `lock_hold` histograms distinguish individual mutex
waits and holds from those frame sums; `lock_holder` and `lock_holder_age_ms`
identify the observed named holder and its age when a waiter arrives. They do
not attribute the entire wait to one holder. The priority queues' `p95_ms` fields still describe total
frame duration, and their depth counts dispatch waiting, not wire admission.
Total duration, per-queue p95, slow-frame `total=`/`queued=` and the 200 ms
slow-frame threshold now measure from intake, including decrypt and admission;
compare them only against this change's baseline, not older logs.
`bridge.stats` answers at intake without an app lock or dispatch worker.

Activity observations begin an `agent.activity` frame before blocking-pool
scheduling, with pool wait reported separately. Self-report discovery runs
outside the app mutex; `ActivityPump::reported` then takes a timed lock. Its
frame ends after snapshot publication, before any required failure-delivery
drain. Status, surface-change and stream-close pumps have separate fixed labels;
their frames cover processing rather than pool scheduling. Broadcast overflow
increments `activity_lagged_reports` and consumption continues with the retained
tail; it does not reconstruct lost events.

The fixed `publication` histograms measure candidate `prepare` (including
`owner_prepare`), save attempts (`persist`), snapshot installation attempts
(`install`, including rejected stale deltas), and nonempty push flushes through
encrypted carrier submission (`push`). `push_pending` measures the oldest
pending batch through successful submission, including coalescing time. These
are batch/stage diagnostics, not per-event browser receipt timestamps. Prepare
spans nest, attempts may fail, and global totals include unrelated producers;
their sums cannot be added as an exclusive event breakdown. Production records
histogram atomics without request-phase observer locks.

An unchanged provider resume name skips its conversation edit only after the
canonical binding, reset reservation and all five session-instance identity
fields match the named lineage. Roster/model updates retain their own mutation
boundaries. A successful canonical activity save carries that owner's snapshot,
so the activity pump does not project it again. Shared conversation owners,
failed saves and complex turn-failure handling retain the explicit owner
projection fallback. This reduces repeated publication without changing commit
atomicity, stale-generation rejection or existing lock-held SQLite paths.

Fine-grained request-phase tracing is test-only: `timing/requests.rs`, its
observer fields and its call sites compile under `cfg(test)`. The non-test
`observation_reads::trace_phase!` expansion contains only the measured work.
Diagnostic test binaries retain that tracer; production retains the ordinary
FrameClock queue/wait/hold aggregates without request-phase observer locks,
frame clones or diagnostic timestamps.

**Independent SQL reads (#681 step 2).** `AppState::handler` captures the
read pool, update-admission service and an atomic immutable `ReadContext` slot.
Producer hooks publish project/path mappings, live owner admission, task
identity/workspace overlays and user presence before their replies. A roster
save adds admission only after success; actual retirement can remove it even
when persistence failed. Durable identity fields come from the read transaction,
so later metadata publication cannot expose a failed roster candidate.
Thread binding, aliases, reset generation, metadata and history share that
transaction. Retained SQL owners remain inaccessible without live admission.
The Store-free fallback is an immutable snapshot. Task timeline, dependency,
review-summary and durable-identity rows share one transaction, with live
availability supplied by the published overlay. Read replies retain typed
parameter normalization and result checks without deferred app state.

**Independent observations (#681 step 4).** `app/observation_reads.rs`
prepares owned jobs from a typed immutable identity context published with the
versioned `ReadView` delta. The context is staged under the existing committed
publication boundary, so failed saves and stale refreshes cannot publish
candidate identities. Preparation never reacquires the app lock or drains
deferred writes. Dedicated I/O workers execute filesystem/Git observations,
then validate source, owner and conversation fences against the latest
publication before returning the existing typed reply. GitHub observations
retain their service deadline and update admission spans the complete job.
Writer routing retains its existing mutation boundaries. MCP read scheduling
is described below under MCP tools.

Ordinary owner/global publication structurally shares unrelated observation
families and rows. Touched owner rows may be recaptured even when their fields
compare equal. Producer mutations identify affected inputs and stage their
family/key values under the committed publication boundary; dirty-key receipts
are acknowledged only after an accepted delta, so failed or stale candidates
retain pending work. Legitimate topology changes may rebuild thin owner/run
order indexes to preserve legacy tie precedence. An unrelated status publication
does not rebuild the complete observation graph or enumerate all retained
lineages, attachment homes and archive rows. Existing board rendering and legacy
producer maintenance remain separate work; this is not a claim that all
publication is constant time.

**Published snapshots (#681 step 3).** `app/read_view.rs` holds an atomic
`Arc<ReadView>` root with shared immutable response chunks. Chunk and history
maps use persistent ordered trees, so retaining a root and applying a small
delta copies bounded tree paths rather than every entry. Versioned deltas
merge disjoint updates through compare-and-swap; stale overlapping updates
are rejected as a unit, and deletion versions remain as tombstones. Producers
warm required observations before admission, then publish committed values
before change events and successful replies. Agent rows use observed paths,
compaction context and unread summaries. Workspace history shares immutable
history chunks; response paging preserves the existing cursor and full-history
contracts without cloning retained history on each publication. Inventory,
update status and CLI catalogs have their own atomic service snapshots.

An incremental publication captures project rows once, borrows those rows for
the structural layout check, then renders affected projects from that same
candidate-local capture. Observed remote and isolation values are still applied
at rendering. Full rebuilds capture independently; failed or stale candidates
retain no project-row cache across publications.

Plan/run finishers stage projections before saving and restore committed
canonical fields after a failed save. Their checkpoints exclude live PTYs;
actual retirement remains observable separately, even when a failed roster
save restores the configured agent. Configured idle agents remain available.
A conversation reset updates every affected owner's checkpoint from the
committed reset payload, so a later failed save cannot resurrect its history.
Permanent owner removal also retires its checkpoint. A failed durable candidate
cannot be exposed by an unrelated later publication. Nested owner mutations
wait until their shared projection can be published coherently. Seen and mute
mutations publish affected agent cursors and unread counts before replying.
New Git adoptions stage their compatibility workspace alongside the owner
before saving. A failed save removes only that staged in-memory alias, while
a successful save publishes both before notices, including branch dispatch
and implementation handover.

Plan/run finishers do not discover or reap unrelated terminals. The periodic
service in `app/runtime/terminals/cleanup.rs` captures registration identities
under a brief app guard, probes checkout roots outside the guard, then applies
each disappearance under its own guard. It rechecks the exact tab role,
session handle and lineage, or waiting-screen handle, before retiring anything.
Cleanup attempts the exact session's lineage and turn closure through its
canonical owner mutation. It publishes actual runtime retirement with committed
roster fields before completing even if that save fails. Known checkout removals
carry captured registrations through
their deferred work and clean up only those covered by a confirmed removal
receipt before replying. Merge prune uses the same registration checks after
successful removal; its existing synchronous filesystem work remains guarded.
Process kill and reap still run separately through `reaper.rs::Retirement`.

User shell creation captures registered scope identity under a brief app guard,
resolves its physical path outside the app mutex, then reserves a terminal slot
and a **per-checkout** lifecycle hold. The PTY opens outside the mutex; another
off-lock path probe and a guarded identity check precede registration. Pending
shells count toward the cap and prevent movement, deletion, locking and reclaim
of their own checkout. Other checkouts remain available. A failed or invalidated
spawn retires its opened child before releasing the reservation.

Terminal open, close and cleanup update terminal rows only for the affected
checkout aliases, including aliases awaiting their first full publication.
They preserve unrelated snapshot chunks and use stored path observations for
notification aliases. Full publications refresh any terminal-only aliases that
remain registered. Opening a scoped terminal retains its workspace/run
interaction stamp and board revision bump. Its attention fields patch the
accepted shared rows without rendering workspace details or a global projection.
An asynchronous store merge preserves newer seen/mute/cursor state and the exact
captured resume boundary; the reply does not wait for this attention save.
Durable run/task rows fence insertion against deletion. External checkout stamps
may insert their first row while the captured full-map save epoch is current; a
later live-map replacement cancels that delayed write. Managed workspace IDs
update only existing rows, matching their exclusion from the old live-map save.
Terminal creation also records user activity and pushes the terminal row. Its
user-session persistence runs outside the app mutex, with a conditional save that
cannot overwrite newer activity. The reply still waits for that save; acknowledgment
before persistence belongs to the separate ack-then-confirm step (6b).

Task creation, updates and dependency edits capture their watched board rows
and affected neighbors inside the SQL transaction. A failed projection read
rolls back the write. Background observation failures retain the previous
rows; initial admission requires an authoritative first observation.

The push bus reads committed state, thread tails, terminal rows and whole
lists from the same immutable view. Git/filesystem observations belong to a
bounded independent producer; a flush does not wait for the app or Store
writer. Source identity fences reject obsolete observations. The existing
250 ms coalescing and wire shapes remain.

[`Bridge Concurrency Plan.md`](<planning/v2/Bridge Concurrency Plan.md>) records
the remaining observation-read and scoped-writer stages. Legacy writers still
serialize through the app mutex and Store writer.

Checkout-root refreshes use the reverse conversation lookup in
`app/workspaces/mod.rs`. It filters candidates against the queried run before
checking a matching workspace's owner, preserving the existing sorted workspace
and run precedence. This avoids scanning every run for every workspace for
every root, with repeated filesystem canonicalization under the app lock.
Counting tests bound the path comparisons for a refresh as runs and workspaces
grow; ownership tests retain direct IDs, terminal states, project conversations
and symlink aliases.
Snapshot preparation creates one ownership context shared by board, workspace,
agent and push projections. Pure ownership indexes can reuse the accepted global
projection's structural data after validating current native paths, observed
aliases, state root, project and parked bindings, exact run states, roster order
and workspace name/ID precedence. Validation still walks the current inputs;
unchanged structure avoids rebuilding and sorting the indexes. Cold observation
state cannot reuse them. Task live locations use a qualified owner index with
the existing project/root checks and project-repository exclusion, so each agent resolves its
location without repeating registry scans. It indexes agent ownership and preferred
run outcomes in the existing precedence order, and reuses branch candidates and
canonical agent digests for matching render inputs. Updated canonical histories
are shared by unread projections and history staging; unread summaries are keyed
by the effective read cursor. Cached external checkout
rows are rendered in one pass and reused by push states, retaining the first
matching project and agent-root entry. Project-scoped workspace filters and
canonical conversation aliases retain their existing behavior without resolving
paths per agent. Board workspace summaries observe in-flight delivery roots
and live session status once for all workspace rows. Push workspace rows join
summaries through a temporary index that preserves JSON id equality and
first-match duplicate precedence. Render memos remain fresh for each outer
guard and end before persistence or canonical mutation. Only an accepted global
delta retains new pure structural data; failed saves and rejected candidates
cannot replace it. Metadata publication still refreshes runtime availability
and saved addressing independently. Adjacent immutable snapshot and read-metadata
publications share that capture within their wrapper, ending before change
notifications or later mutations. Complete local read-view deltas retain their atomic publication
boundary before replies and change notices. Counting tests include the
publication producer as well as the pure push reader.
Bounded push tail bodies are shared between publications while conversation
identity, publication sequence and resident tail geometry match. Each publication
still captures fresh tip metadata. Historical receipt rows retain their existing
bounded floor and generation fences.

Global board and list projections retain shared row fragments in the accepted
`ReadView`, alongside the layout inputs that justify reusing them. Known-owner
and committed task-observation publications replace affected rows; structural
changes use the full producer. Alias and shared-root dependencies, active
session observations, and elapsed-time fields participate in each refresh.
Registry identities use weak handles so a retained read root cannot keep a
retired session or terminal screen alive. Attention changes for known owners
use the same scoped publication; other board invalidations retain their full
refresh path. Layout reuse compares raw committed compaction policies with
workspace and project topology rather than rendering every inherited policy to
check for changes. Workspace detail refresh batches affected owners and aliases
through one registry and dependency scan. Fresh base rows and agent variants
share their unchanged JSON subtrees; dependency scans borrow fields from either
composed or legacy rows without materializing an aggregate. Active-owner and
elapsed-time fields are still rendered afresh on each publication.

Each incremental refresh discovers current active owners once and builds one
owner dependency index. Two independent closure walks compute dirty owners and
the next active set. Previous active owners and explicit state/stat changes join
the dirty set only; an idle owner receives its cleanup refresh without staying
active. Failed saves retain pending-owner retries until an accepted publication.

Within one candidate, an incremental board winner and its push state share the
same normalized branch wire row, keyed by owner and exact stat input. The raw
fold candidate remains separate, and losing owners retain their own push state.
This memo ends with the projection guard; full seeds keep their independent
construction. Observation scope installation still replaces every supplied run
input and updates lineage, but edits workspace membership only when the
workspace ID changes. Physical paths and other changed fields still install.

Within `StatusPump::observe`, attention persistence and dirty marks still
update immediately, while their wire projection joins the final owner
publication. Changed attention refreshes read metadata with that final
publication, using one fresh ownership capture after all later helpers have
finished. A `working_since` roster save keeps its independent committed
publication and admission update. Direct status APIs and end-of-stream handling
keep immediate attention publication. Context, reset and usage helpers keep
their existing save and failure behavior.

An RPC `thread.post` keeps its durable message publication and both attention
saves, but publishes the two human attention stamps together before returning
from its dispatch guard. The final owner delta includes every addressed and
canonical alias. A route error still publishes attention from any committed
prefix; a deferred job receives that publication before leaving the guard.
Successful interaction stamping retains its existing timing, including after
deferred completion. Direct and MCP posts keep immediate attention publication.
The temporary collection restores its state on unwind without publishing a
partially mutated candidate, and never spans external work or delivery drain.

Deferred status and RPC attention stamps retain revision-tagged notification
receipts for each affected owner and alias. Their ordinary early notices remain;
an accepted publication also emits fresh entity and board notices, so a flush
that consumed the early notices cannot leave subscribers on the old state.
Incremental candidates acknowledge only owners they refreshed, while a complete
candidate can acknowledge removed owners. Rejected or borrowed-owner candidates
retain their receipts, and an older accepted candidate cannot erase a newer
stamp. Notification receipts are independent of observation-capture receipts.

`read_view/shared_json.rs` composes immutable JSON leaves without caching a
materialized aggregate. Intake and push readers materialize their owned wire
answers after releasing the app guard. Historical receipt patches index exact
thread identities and replace only advancing leaves, updating cached row
fragments in the same atomic delta. Preparing or rejecting a candidate cannot
change the accepted fragments, and direct aggregate replacements invalidate
their reuse cache. Watched task observations retain unchanged payloads by `Arc`.

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
  `3.24.0`. `fixtures/api/versions.json` (`"current"`) must match it.
  3.24.0 adds the durable write foundation: `bridge.epoch`, bounded
  `writes.status`/`writes.subscribe`, and typed ack/confirmation envelopes.
  Every authenticated outgoing application frame is stamped with the same
  per-process UUID at `SessionSender::seal`, including replies, receipts,
  terminal/service pushes and authenticated signaling. `session.hello` returns
  that `bridge_epoch` and the persistent `journal_incarnation`. Planned update
  handoff emits a best-effort `bridge.restarting`; reconnect greeting remains
  authoritative when that event is lost.
  Hello advertises `ack_confirm_methods`, mirroring
  `api::writes::ACK_CONFIRM_METHODS`; it is empty in #739. Clients use
  ack/confirm only for methods in that list. There is no `write.mode` or
  separate ack-confirm capability negotiation.
  Bridge and SPA ship together; #740 raises the SPA requirement to this version
  under Zech’s 2026-10-21 decision. Shared envelopes are in
  `fixtures/api/v1/writes/`; their Rust contracts live in `api/writes.rs`.
  3.23.0 populates the existing `attachments` descriptors on agent replies,
  outgoing agent messages and the corresponding incoming messages (#696).
  The thread page fixture shows all three. No new capability is needed:
  the SPA paints the descriptors present in its cached thread records.
  `thread.attachment` now caps an unranged response at 5,242,880 bytes too.
  Its `size` remains the total file length and `offset` names the chunk start.
  If `offset + decoded byte count < size`, the client must request the next
  range at that sum before using the file. The SPA's shared attachment reader follows that metadata even before
  it knows the paging capability, and checks offsets, size and progress.
  User uploads remain capped at 5,242,880 bytes by `ATTACHMENT_MAX_BYTES`
  and the size check in `app/conversations/attachments.rs::thread_attach`.
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
  3.13.0 adds `fs.createDirectory`, `fs.uploadBegin`, `fs.uploadChunk`,
  `fs.uploadFinish` and `fs.uploadAbort` (#391), each announced by its verb
  name, plus the nonretryable `already_exists` error code. The new mutations
  create directories and stage binary uploads inside a resolved file scope.
  3.14.0 adds `workspace.set_locked` (#398), announced by its verb name,
  the Boolean `locked` field on workspace rows, and the nonretryable `locked`
  error code for removals held by the user's workspace lock.
  3.15.0 publishes `tasks.review.open`, `push`, `update`, `merge`, `close`,
  `reopen` and `refresh`, under the `tasks.review.` namespace (#405). Each is
  announced by its verb name; `tasks.review.pullRequests` separately announces PR
  mode, lifecycle, summaries and sync observations. Legacy Snapshot review
  verbs retain their contracts and individual capabilities. See Task reviews.
  3.16.0 adds `fs.createFile` (#433), announced by its verb name: one empty
  regular file in a resolved file scope, with `fs.createDirectory`'s fence and
  `already_exists` refusal. `fs.write` still only replaces a file it has read.
  3.17.0 adds `fs.projectSourcePath` (#360): `fs.tree`, `fs.read`,
  `fs.write`, `fs.createDirectory`, `fs.createFile` and `fs.uploadBegin`
  accept `source_path` beside `project_id` plus `source_id`, the path the
  source row carried when the client opened it. If the source has since moved,
  the bridge refuses the call with `conflict` instead of reaching the new
  folder. Beside any other scope it is `invalid_params`.
  3.20.0 adds `threads.leanActivity` (#496): optional `activity: "newest"`
  on `thread.page` ships one newest item per activity run, including the live
  tail, beside an exact full-run digest. Omitting `activity` keeps the bounded
  default. The mode applies to backward pages, forward deltas and newest
  deltas; `thread.activity` still reads the digest's span on demand.
  3.21.0 adds `git.projectSources` and `changes.projectSources` (#529):
  the `git.*` verbs take `project_id` + `source_id` + a required
  `source_path` for one original project source, and `changes.subscribe`
  takes scope `{kind: "project_source", project_id, source_id, source_path}`,
  whose items carry `project_source`. See Projects and sources.
  3.21.0 adds `sources.syncBasePath` (#530): optional captured `source_path`
  on `project.sync_source`, checked on admission and after queue/lock waits.
  3.21.0 adds `agents.projectWorkers` (#532): `agent.list {entity_id,
  include_project_workers}` (a verb again, after 3.0 retired the old one),
  `project_role` (`coordinator` or `worker`) on a project owner's agent
  digests, `pinned_agent_id` on its `agent.list` answer, and `project_role`
  on `agent.add`, which accepts only `"worker"` and only on a project's
  conversation owner. Every roster read that predates workers (board rows,
  the project list's conversations, `agent.remove`, and `agent.list` without
  the flag) carries a project owner's pinned coordinator alone. Task agent
  identities and inline task-notice identities also carry optional
  `project_role` (#536), retained independently of an optional name or live
  membership. This additive field shares the unreleased 3.21.0 minor; clients
  read it when present and need no separate dispatch capability. Without the
  field or exact cached pin membership, a captured project identity stays a
  generic agent with its saved ordinal/harness rather than claiming to be the
  coordinator. The `project_identities` contract entries on `tasks.get`,
  `tasks.list` and `thread.page` record the field and its `since: 3.21.0`.
  See MCP tools. 3.21.0 also adds `tasks.assignProjectAgent` (#533): the
  `new_project_agent` assignee kind on `tasks.assign` and `tasks.create`,
  and `operation_id` on `tasks.assign`, which a retry answers with the
  dispatch already made. A reused id asked of a different assignment is
  `conflict`.
  3.21.0 also adds `tasks.activitySessions` (#628): watched task records and
  board rows carry `session_started_ms` and `last_activity_ms` for the current
  eight-hour-gap activity streak, including silent content edits. Conversation
  sessions use that same gap; the user's device-wide session retains six hours.
  3.19.0 adds `unread.watchScoped` (#474): conversation and task unread
  counts exclude unwatched conversations and news from before the current
  watch began. Watch baselines persist separately from read cursors.
  3.18.0 adds the harness inventory (#434): `harnesses.list` and
  `harnesses.refresh`, each announced by its verb name, and the
  `harnesses.changed` push. See Harnesses and the agents' slice.
  3.11.0 adds `conversation.reset` (#358) and thread generations on conversation
  digests and responses. Generation-aware requests refuse a cleared thread;
  the reset capability gates the menu, its generation-aware cache handling,
  and the added request parameters on existing conversation verbs.
  The SPA's adapter claims `>=2.0.0 <4.0.0`: it calls nothing a 2.x bridge
  lacks (what 2.x added after 2.0.0 is capability-gated), so the app can
  roll before the bridge.
  3.21.0 also adds `tasks.waitingOn` (#558/#562), sharing the still-unreleased
  0.2.13 release: create-time dependencies, `tasks.waiting_on.add/remove/blocks`,
  and `retry_deferred_id` on assignment. Task answers carry graph-owned
  `dependencies` (forward resolution reasons, authoritative blocking, reverse
  count and a bounded detail preview) and saved `deferred_assignment` state.
  A closed prerequisite resolves with `resolved_by: "close"`; Done resolves
  with `"done"`. Closure briefs ask the recipient to assess and escalate on
  the task when unclear. The retired `tasks.link` public RPC stays retired.
  The SPA persists whether each device has ever advertised waiting-on support.
  A later greeting without it marks a downgraded bridge: ordinary new-workspace
  assignment is refused until current support returns. Never-upgraded devices
  retain legacy assignment; capable devices enforce prerequisites themselves.
- `session.hello` is answered by `session_hello` in
  `bridge/src/app/runtime/terminals.rs`. The reply carries `api_version`,
  `capabilities`, `push_events`, `events` and the `changes` subscription settings.
  It also records what the client said it was (`bridge/src/api/clients.rs`).
  Wire 3.22.0 adds `compaction.harnessThresholds`, the provider-map patch on
  `settings.set`, and typed `project.set_compaction` / `workspace.set_compaction`.
  Settings, project and workspace records carry raw `compaction_thresholds`
  and per-provider `compaction.inherited` / `compaction.effective` observations.
  Agent digests carry configured, inherited and active policy separately;
  `conversation.settings` also returns inheritance while overridden. The new
  `compaction.changed` device event carries only `scope` and `owner_id`.
  A client opts in through `changes.subscribe`'s `compaction.changed` kind
  only when the current greeting advertises the feature. Refreshing after
  subscription acceptance closes the missed-invalidation gap.
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
  open lands the linked conversation on its unread line, just like every
  other chat opening (including switches, reloads and background resumes).
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

Delivered operation receipts retain their request hash, owner/agent identity and
message sequence span, but discard their delivery prompt in the status update.
Ordinary receipts clear `delivery` to NULL; historical Task-to-implementation
forwarding retains a small target intent with no payload when its execution
owner differs from the original receipt address.
Queued, claimed and unresolved uncertain receipts retain their payload for
recovery. A delivered receipt later marked uncertain remains payload-free and
cannot replay. Sent, seen, failure and completion projection readers use receipt metadata and stored
messages after delivery; boot recovery uses the receipt span even without a
payload. The remaining receipt is an idempotency tombstone; retaining its numeric
span does not require retaining the referenced transcript rows.

Before publishing its read pool, Store startup converts older databases to
`auto_vacuum=INCREMENTAL` with SQLite's transactional `VACUUM`; a failed conversion
fails startup before serving requests. This one-time rewrite needs temporary
space for the database. A startup-only WAL checkpoint/truncate returns the
rewrite log space before reader admission; a pending marker makes an interrupted
checkpoint retry on the next open. The daemon's storage maintenance worker uses the existing
writer outside the app lock. It clears old delivered payloads with resumable
rowid-keyset batches of at most 1,000 receipts and reclaims at most 32 free pages
per step. Cleanup progress is committed with each batch; later steps retain only
small incremental-vacuum work. Readers continue on their independent WAL pool.
Shutdown records the restart roster and tears down children before cancelling
maintenance. Its worker join has a one-second budget; the main Tokio runtime
also has a one-second shutdown timeout so a blocking batch waiting on the writer
cannot keep the process alive indefinitely. A detached batch may finish during
that grace period; its transactions remain atomic and cleanup remains resumable.

#### Durable write foundation (#739)

Schema 18 adds `mutation_commands`, per-owner ordering, dependency edges,
resource reservations and a terminal outbox. `StoreWriter` owns the same
connection used by existing Store methods; no second writer is opened.
New journal admission is a short SQL-only transaction entered outside the app
and domain guards. Existing Store callers are converted incrementally; this
foundation does not claim to have removed their existing lock-held SQL.

A command is keyed by the server-derived principal and a canonical operation
UUID. The normalized semantic intent and saved dependencies fence conflicting
retries. Admission reserves identities and durable owner order before returning.
Canonical SQL and the committed marker share one transaction; publication and
terminal confirmation follow. Reopening the intact store retains its journal
incarnation and outbox. SQL-only claimed work can return to accepted, while
ambiguous external claims retain an unknown outcome and block conflicting
execution until reconciliation. Committed work is returned for the enabled
verb’s publication recovery and is never applied twice. No verb executor is
enabled by this slice. The journal exposes 30-day terminal-result compaction for the future enabled
command service; identity/outcome tombstones remain, and unresolved partial/unknown
evidence is retained until reconciliation. Active owner fences and the
principal/cursor outbox index keep settled lifetime history out of claim and
caught-up subscription queries. Admission limits are 128 nonterminal commands
per principal, 4,096 per store and 64 KiB per admission payload; terminal outcomes
are limited to 256 KiB, checked before committing canonical SQL.

`app/mutations.rs` reads journal status using the existing independent SQL lane
and read-only pool. Status accepts at most 128 operation IDs and returns at most
512 KiB per page with a cursor bound to the operation set and journal incarnation.
Subscribe captures the current outbox cursor and states in one SQL snapshot,
then replays later durable outcomes outside ordinary change coalescing. Readers
never take the app guard or writer connection. Current RPC authorization is the
paired device account, represented by a server-owned database-local principal;
client hints and session IDs do not grant identity. Reads additionally validate
the original owner scope against current published authority. MCP agent
principals and semantic method enablement belong to Step 6b.

#### Conversation retention (#836)

`retention` in the persisted device config defaults to
`{"enabled":true,"min_messages":50,"max_messages":100,"window_days":7}`.
For each conversation, keep the newest N messages, where N is the number
from the last seven days clamped to 50–100. Fewer than 50 total messages all
survive. Activity survives at and after the oldest kept message's creation
sequence. Tasks, task comments and reviews are independent and never pruned.
Invalid explicit config refuses startup rather than enabling a guessed policy.
Changes to this config take effect after restart.

The daemon runs an immediate startup cycle and repeats hourly. Discovery uses
keyset pages; the store commits at most 1,000 deletions per batch outside the
app lock. The earliest span start of queued, claimed or uncertain operations
caps the prune floor: older items remain prunable, while the unresolved span
and every newer item survive, even when that exceeds the configured maximum.
Origin and canonical forwarded conversations are
protected. Pending legacy messages cap the floor at their creation sequence;
missing operation span information requires conservative protection.
Settled operation rows remain as small payload-free idempotency tombstones,
retaining identity, request hash, original numeric sequence spans, status and
outcome metadata. Retaining these receipts instead of deleting them preserves
retry deduplication and late-status readers without pinning transcript history.
Forwarded receipts may retain a payload-free routing stub. #835 owns payload
compaction and the sole incremental-vacuum scheduler.

Schema 19 stores a generation-scoped `conversation_retention` floor and sequence
high-water mark. A bounded timestamp-only summary of pruned recent messages
preserves the week count when imported timestamps are out of sequence; batches
and later cycles cannot shrink the target by deleting their own counting evidence.
The summary holds at most max_messages Julian-day values, expires logically with
the configured window, and resets with the conversation. Stale resident saves
cannot reinsert rows below the floor.
`thread.page`, `thread.activity` and push tips expose optional
`history_start_sequence`, the inclusive retained creation-sequence floor.
Absent means no declared trim. The SPA discards the removed prefix and its
repair targets, preserves the maximum floor only within one generation, and
continues repairing any gaps above the floor. Reset discards the previous floor.
An empty history can report high-water mark + 1 without reissuing counters.
Validated repair floors survive a later failed exact read. Retry exhaustion is
bounded by counter and monotonic floor: a higher floor renews the same-counter
budget, while identical or lower floors do not. Coalesced floor advances retain
the largest queued suffix counter and never certify gaps in retained history.

Attachment sweeping runs independently of the pruning toggle. Exact parsed
path references across durable documents and draft upload reservations protect
both local and checkout copies. Message commits do not consume shared-path
reservations: another client may still hold that file in a draft. Stale
reservations expire after the configured window; newly migrated reservations receive a full grace window. File scans run
without the writer or app lock. A bounded final unlink rechecks the unchanged
DB snapshot and shares an admission gate with file validation, which refreshes
mtime. Files newer than one hour, symlinks and directories are left alone.
Concurrent DB changes defer that sweep batch to a later cycle.

Set `retention.enabled=false` for hourly count-only history reports while leaving
orphan cleanup active. For an offline detailed report, copy the tasks directory
first, then run `cargo run --example retention_report -- <copied-tasks-directory>`.
Opening that copy initializes its schema; it never starts the daemon or prunes.

#### Compaction policy configuration

`bridge/src/compaction.rs` defines sparse per-harness token maps, and
`bridge/src/app/config/compaction.rs` owns their persistence and resolution.
The independent keys are `claude`, `claude_adk`, `codex`, `codex_app_server`,
and `pi`. New values must be positive safe integers (at most 9007199254740991).
An omitted patch key is unchanged; a provider's `null` removes its override.
An empty patch does nothing. Unknown providers, invalid numbers and mixed
scopes are refused before persistence. Each mutation has one typed scope and
writes a prospective config atomically before changing memory.

Device `compaction_thresholds` lives at the config root, project
`compaction_thresholds` on the project entry, and
`workspace_compaction_thresholds[workspace_id]` at the root. Workspace maps
never enter checkout manifests or adopted source directories. Stable owner
IDs preserve policy across rename, restart and parked projects. Only confirmed
owner deletion removes its maps; an unavailable project or checkout is not a
deletion. Pruning an absent adopted checkout's row retains its policy, so
re-adoption at the same deterministic checkout ID recovers the saved settings.
Workspace deletion preflights the current config write before removing files.
After confirmed deletion, policy cleanup is best effort: a late persistence
failure logs a warning and returns an internal retained-settings outcome
without changing the successful deletion response. Such orphan maps remain
on disk; re-adopting the same external checkout path can reuse its deterministic
ID and retained policy. Startup adoption and identity rules are unchanged.

The internal `AppState::resolve_compaction_policy` resolves
conversation override → workspace → project → device → 200000, including the
winning source. The addressed agent's launch choice selects the harness key.
The canonical conversation owns `max_context_tokens` and the workspace/project
scopes: alias reads and writes share that override, while aliases launching
different harnesses select different entries from the same scopes. Project
coordinators and workers skip workspace scope. Spawn, routing, model choice,
and session ownership remain addressed-agent behavior. This resolver reports
configured intent; it does not apply or report an active harness policy.

Loading a config without a device map seeds all five entries from the legacy
`compact_above_tokens`, substituting 200000 for zero or an absent value.
An explicitly empty map is never reseeded. Agent JSON loading clears a legacy
`max_context_tokens` zero to inheritance. The legacy device scalar is retained,
with an absent or zero value normalized to 200000 on load, including runtime
checks, settings/digest reads and subsequent persistence. Runtime threshold
resolution also treats a legacy conversation zero as inheritance and a device
zero as 200000; zero no longer disables automatic compaction.
An old scalar write broadcasts a positive safe integer to all five device
entries and leaves narrower scopes alone. Combining a scalar and map mutation
is refused. The legacy read scalar is the normalized legacy value or the last
scalar broadcast; it cannot describe divergent harness entries or narrower
scope maps. Config preserves out-of-range
legacy integers exactly; internal read maps use `{tokens_decimal: "…"}`, and
resolved observations use `tokens: null` with that exact decimal and an
`unsupported_representation` reason. Those representations cannot be submitted
as mutations. Older bridge binaries can discard the new maps when rewriting
config; downgrade preservation is unsupported.

Wire 3.22.0 exposes these maps through `settings.get/set`, project records and
workspace records. `project.set_compaction` takes `project_id` and
`compaction_thresholds`; `workspace.set_compaction` takes `workspace_id` and
the same patch. Each returns its authoritative scoped record. Unknown params
and providers are refused, as are zero and unsafe mutation values. Device
settings reject mixing the legacy scalar and map. The legacy scalar read
retains its compatibility limitation; new clients read the resolved maps.
Successful policy changes emit `compaction.changed` as a scope/owner
invalidation to opted-in clients. No settings replica travels in that event.
The event causes authoritative scope and affected held roster reads; ordinary
state invalidations continue to carry session-open policy changes.

#### Compaction session policy

`bridge/src/harness/compaction.rs` owns the wrapper contract. Each admission
captures the resolver's positive token threshold and source, the queued
launch's frozen model/provider choice, the canonical thread generation and a
unique launch identity. `AgentLaunch`, `SpawnOptions`, `PreparedAgentLaunch`
and `SessionOpenRequest` carry that same value through off-lock preparation.
A settings edit during preparation stays pending for the next opening.

The common harness opener decorates the opened session with `PolicySession`.
Its active `AppliedCompactionPolicy` is unavailable until the carrier confirms
a successful start or resume: ADK's init announcement with no startup refusal
and Codex app-server's accepted thread response are the protocol boundaries,
independent of whether an early send already reports Working. Failed launches
install no active policy. Publication checks the captured thread generation before touching
newer runtime settings, tokens or waiting clients. Capture routers have no
canonical conversation and retain their existing launch behavior without a
policy snapshot.

The snapshot records requested tokens/source, any qualified native trigger,
enforcement (`native`, `between_turn`, `unsupported`), the launch choice and
identity, and version/evidence/reason. Capability descriptors include native
threshold and between-turn command support plus the native unit/range when
qualified. Native adapters remain unqualified. Claude and Codex carriers use
the existing between-turn command path; Pi qualifies its extension-backed
fallback only for 0.86.1. Unknown or failed version readings never infer
native support. Unsafe legacy values remain exact and unsupported; zero is
refused. Wire agent `compaction` observations expose configured and inherited
policy independently from the active snapshot. An unknown active policy stays
null, with unsupported enforcement and an explicit reason; pending change is
unknown until there is a snapshot to compare. A save never substitutes the
configured policy for active evidence. Qualified active snapshots report the
policy applied at start and its harness qualification, rather
than claiming a live native readback.

Both Claude carriers share `harness/claude_compaction.rs` (#656). Formula
revision 1 targets the inspected 2.1.284 explicit window: reserve is the smaller
of the effective output maximum and 20000; the trigger is window minus reserve
minus 13000, optionally lowered by a valid percentage of usable context.
Small thresholds use a legal 100k window and a decimal percentage whose actual
round trip preserves the requested count. The model window clamps the usable
window, never the requested policy. The adapter refuses unknown versions,
unknown budgets and targets above the native trigger. A captured optional
`NativeCompactionSettings` carries a translated window/percentage so spec
construction cannot recompute it from later settings.

Production Claude native promotion is deliberately closed: the requested
model and observed CLI version cannot bound settings-sourced output reserves,
managed model remapping or in-turn model fallback. Both carriers report a
reason and use between-turn enforcement. Their specs merge only compaction
controls into their existing hook `--settings`, including `settings.env`,
because Claude applies that environment after inherited child environment.
Fallback sets child-only `DISABLE_AUTO_COMPACT=1` and `DISABLE_COMPACT=0`, clears
stale window/percentage overrides, and leaves manual `/compact` available.
Output limits, selected model IDs (including `[1m]`), ordinary settings,
credentials and parent environment are unchanged. Policy-free routers keep
their original specs.

Managed settings outrank flag settings. Bounded read-only inspection of local
Linux managed files, fragments and the remote cache names observed conflicting
keys without exposing values. Such fallback is explicitly **best effort,
managed policy present**; unreadable, dynamic and unverified platform policy
also reports its limit. This is a launch-time local observation, not a claim
about subsequent remote policy refreshes. No policy helper is executed. These
bounded reads currently run under the app lock as part of admission, where the
complete immutable policy is captured; moving them off-lock requires separating
captured inputs from adapter preparation. The qualification record is
`planning/v2/Claude Compaction Qualification.md`:
four isolated 2.1.284 model runs observed successful resumed auto-compaction,
but did not establish production budget/fallback invariants.

ADK recognizes both PreCompact hooks and 2.1.284 `status=compacting` as one
compaction start. Only `compact_boundary` establishes completion and changed
context. Status and hook events do not finish the ongoing turn or consume
queued input; failed attempts cannot report successful compaction. The Claude
TUI reports start and completion through its generated PreCompact/PostCompact
commands and the compaction sidecar. Its fake-child integration test runs those
commands through the sidecar follower and activity pump during an open turn:
PreCompact and terminal text cannot establish success, PostCompact completes the
compaction row, and both leave the session working and its queued follow-up
untouched. After an explicit turn end, the real delivery queue sends that
follow-up once without injecting a Build threshold command under Native.

Both Codex carriers share `harness/codex_compaction.rs` (#657). Translation
revision 1 targets 0.160.0: `model_auto_compact_token_limit=T` with explicit
`model_auto_compact_token_limit_scope="total"`, bounded by the native ceiling
`floor(9 * raw_model_window / 10)`. This is distinct from the usual 95% usable
context window. The translator refuses zero, unsafe integers, overflowing
upstream window arithmetic and thresholds beyond that ceiling. Exact trigger
qualification also requires an established usable-context cap, zero fallback
buffer and compatible post-turn settings; no model/window is inferred from a
version or catalog name. Both carriers currently report explicit between-turn
fallback. Codex's native defaults remain active; there is no native-disable
mechanism or claim of exclusive fallback control.

A captured Codex native translation, when qualified, normalizes only the two
compaction overrides in each child's argv. The TUI applies them before fresh,
exact-resume or `resume --last` selection. App-server also stores the same
immutable config per connection and includes it only on `thread/start` and
`thread/resume`. Both specs reject a mismatched launch choice or malformed
native snapshot; app-server validates again when opening its thread. The TUI
uses the existing immutable prepared spec and default terminal opener.
User/project configuration files, `model_context_window`,
parent environment, routing and session ownership are untouched. Qualification
in `planning/v2/Codex Compaction Qualification.md` records isolated app-server
start/resume and TUI resume compaction, including a mid-turn tool continuation,
and the dynamic model/buffer/settings limits that keep production native off.
Selected-profile precedence is another explicit promotion blocker. A
credential-free 0.160.0 `config/read` probe rejects legacy `profile`/`profiles`
tables; its suggested `--profile x` file form is disallowed for app-server.
Ordinary-file overrides are established, but supported runtime-profile and
per-thread profile precedence remain unverified. Profile fixtures retain
positive fallback and unchanged files; they do not simulate a native override
of rejected legacy settings. Fallback does not repair those settings.
Both selected-profile forms retain the known-version reason
`codex_model_window_buffer_and_profile_unqualified`. Since the captured launch
does not establish profile absence, this reason also applies to other 0.160.0
openings without reading user configuration.

Codex app-server compaction items and the TUI's persisted `compacted` records
remain activity within an open turn. Fake children exercise both through the
real runtime pumps and delivery queue: failed items, foreign sessions, old
history and terminal banners cannot report success; successful compaction keeps
the session working and queued follow-up intact. After the turn ends the
follow-up is delivered exactly once, without Build threshold compaction under
Native. Fresh/resumed config fixtures independently preserve stale file bytes
and isolate concurrent sessions' captured thresholds.

Pi's adapter (`harness/pi_compaction.rs`, #659) passes the captured positive
policy and exact owner/thread/launch identities only to its child extension.
It leaves user and workspace settings alone. The existing private session
directory and `--session-id <owner>` preserve exact continuation; each opening
gets a new private telemetry file. Unknown versions remain unsupported with
`pi_version_unqualified` and retain the existing terminal lifecycle without
loading unqualified observation hooks. Native threshold injection remains unqualified
(`pi_native_threshold_injection_unqualified`); it is not needed for the
qualified between-turn fallback. `planning/v2/Pi Compaction Qualification.md`
records the pinned 0.86.1 executable, documented hooks and credential-free
synthetic-provider runs through the real TUI and unmodified default export,
with a fake MCP endpoint in an isolated child environment. The supplemental
RPC probe is hook evidence only. Promotion blockers remain for other Pi
versions and native threshold injection; neither is claimed supported.
Live-provider summary quality also remains unqualified: the carrier evidence
uses a synthetic provider and does not promote any external provider.

The extension cancels only native `threshold` attempts while fallback owns
the threshold. Native `overflow` recovery and manual compaction remain
available. Context and working observations use the private sidecar, never
screen scraping or shared configuration. `PtySession` consumes these through
`pi_telemetry.rs`; it retains the terminal escape hatch but stops inferring
idle from silence while telemetry is healthy. The session exposes explicit
unsupported states until the extension confirms its exact Pi session identity,
and after extension or telemetry failure. Those failures never kill Pi or
close a healthy MCP client: Build stops threshold compaction, clears stale
context once per generation, and uses the ordinary terminal lifecycle for
continued work. Build tools remain available. Actual MCP transport failure is
different: the extension reaps its owned MCP child and signals Pi with SIGTERM
so the normal tab-exit path can respawn it with working tools, including when
no compaction policy exists. The shared policy decorator preserves the explicit
unsupported reason for compaction observation failure.

A startup record and one-second heartbeats are independent of MCP initialization.
The initial observation budget is 90 seconds, covering both 30-second MCP
requests; a ten-second heartbeat gap withdraws support even if a broken sink
cannot report its own failure. Threshold cancellation requires a fresh receipt
from the Rust observer in a private, atomically replaced `.ack` sidecar. The
receipt binds owner, thread, launch, generation and the last sequence actually
read; only a ready observer can authorize cancellation. Reading the same receipt
again cannot renew its eight-second monotonic lease, which is bounded by the
original record emission time rather than the receipt arrival time. Thus successful writes
without a working reader cannot suppress native threshold compaction. Failure
withdraws the receipt once; a valid new generation can requalify. The extension
also rejects a stalled local writer, without closing healthy Build tools.
Each extension reload advances a launch-scoped
generation and restarts its sequence at one. The bridge requires a new starting
record, resets context and readiness, and rejects records from retired
generations. Foreign launch identities, replayed sequences and malformed or
missing records cannot authorize fallback. Terminal EOF drains final complete
records without publishing another idle boundary, then closes activity even
while the ended session remains available in the terminal.

Ordinary Pi work becomes idle only at `agent_settled`, after retries, tool work,
automatic compaction and queued continuations. `agent_end` and `turn_end` only
refresh context. Idle manual compaction does not emit `agent_settled` in
0.86.1: its completion hook schedules a macrotask that verifies idle with no
pending continuation before reporting settlement. A newer work event defeats
that deferred check. Build rejects a `/compact` send without a confirmed idle
boundary; the command itself immediately reserves that boundary as working.
Pi owns the observed start row, so the shared send path does not append a
second synthetic start. Only the observed `session_compact` boundary can
complete an attempted compaction. Failure, abort and cancellation never do. Overflow compaction
remains activity within the ongoing turn and is identified as overflow
recovery, not enforcement of Build's configured threshold.

Null context clears the session reading and advances a cumulative invalidation
counter. The shared status pump clears the persisted reading and its dedup
state even when watch updates coalesce; an identical later, fresh reading is
then observable again. Pi additionally requires the persisted reading to match
its current observation before declaring automatic compaction due. The common
ledger and queue deliver deferred input once after compaction settles.

Automatic delivery asks the live session policy whether its last trustworthy
context reading reached the captured threshold (`>=`). Only an idle warm
session is eligible; unknown readings and legacy sessions without a snapshot
never schedule threshold compaction. Native mode never schedules a duplicate
Build threshold command. The existing ledger, manual/agent-requested commands,
queue and consumption receipts retain their behavior. A successful send spends
its reading once. Failed/cancelled/interrupted native compaction items are not
successes and cannot clear context or consume a pending receipt; a successful
native compaction is activity within the ongoing turn.

Actual reopening, including recovery after process loss or bridge restart,
resolves the policy again. Exact resume preserves the conversation's saved
context reading and timestamp, including for an idle reopen or bridge recovery.
That reading remains available for the context gauge, threshold decisions and
compaction before/after reporting. A resume probe that establishes the recorded
session is gone clears its reading when the replacement starts fresh;
conversation reset also clears it. Warm turns, idle demotion (which retains
the live session) and browser attach keep their policy snapshot. Native model
changes require requalification at an opening; fallback uses its unchanged
token threshold across a supported in-place model change.

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
A configured project whose home folder is missing at boot is parked, not
dropped (`bridge/src/app/projects/parked.rs`): its configuration is kept
verbatim and written back on every save, its `proj-N` stays reserved, and
whenever something needs it — a worker starting, `project.list`,
`project.ensure_conversation`, `project.create`, the source-sync timer — the
bridge looks again, loads it under its own id, and binds the owners a boot
could not bind, so its conversation, pin and workers answer unrepaired.
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
save is dispatched, so moving a folder cannot redirect that draft's save. On
a bridge that announces `fs.projectSourcePath` the SPA also sends that
captured location as `source_path`, so the bridge refuses a stale scope
itself even when the client's copy of the project is out of date. A
project source's root resolves to its canonical path, as a workspace
source's does.
The Git verbs reach each original source the same way (#529): every `git.*`
verb a Changes pane calls takes `project_id` + `source_id` + `source_path`,
announced as `git.projectSources`. `source_path` is required there, and
membership and the moved-source check come from one resolver shared with
Files (`resolve_project_source`, `bridge/src/app/projects/source_scope.rs`).
A moved source answers `conflict`; a plain folder answers `invalid_params`
("not a Git repository"), so no Git verb reaches some enclosing repository.
Every key a source scope answers is namespaced by project, source and
captured path: `status_key` and each status file's `content_key` (`git.status`,
the status every mutation answers, and the pushed `git` item), the
`content_key`s of `git.diff`, `git.changeset_diff` and `git.unpushed`, and
`git.unpushed`'s `diff_key` and the pushed `unpushed.diff_key`. No other Git
answer carries a key. So two sources with identical changes never share a
cache key, and a moved source starts a new namespace. A workspace directory's
unpublished keys stay raw, because its narrowed `git.changeset_diff` has to
agree with them. History marks nothing,
as the primary checkout's does; `GitScope::marks_unpushed` chooses a
workspace directory's publication marking explicitly. `git.changeset_diff`
reads a source's uncommitted work, like the primary checkout's. Legacy
`{project_id}` still reaches the first source with its established keys.
The same three fields name a source to `changes.subscribe` (scope kind
`project_source`, `changes.projectSources`). Every source, Git or plain, is a
watched entity keyed `project-source:` + a digest of project, source and
captured path (`project_source_entity_id`, `bridge/src/changes.rs`); its
items carry `project_source` and the keys its own Git verbs answer. Scope
`all` never covers those entities, so an older client hears exactly what it
did. A subscription given a new scope (another folder, `all`, an entity, the
board) drops what it held for the old one. A frame already taken is sealed
off-lock and sent under the bus lock only if the very subscription that
took it is still there (an instance minted per subscribe, so an id
unsubscribed and subscribed again is a new one) and its scope epoch still
matches. So once the re-scoping `changes.subscribe` is applied, no
old-scope frame follows it. A source moved or removed in settings leaves the
roots snapshot and its watcher stops. Reconcile passes apply their snapshots
in the order they read them, so a stale pass cannot restart that watcher.
The facts source checks immutable source identity; moved or removed sources
come back `withdrawn`, and their items are dropped. The independent observation
producer rechecks that identity before publishing its off-lock reads.
A frame whose facts passed that check just before the move
can still go out just after it. It carries what the old folder held before
the move, under the old `project_source`. Nothing collected after the move
goes out for the old folder. `project.sync_source` also accepts an optional
captured `source_path` (`sources.syncBasePath`, wire 3.21.0, #530). A manual
request captures the registered location and configured base through the
service queue; after acquiring the source lock the service rechecks both,
refusing a moved source rather than syncing either its old or new folder.

`term.create`, `term.list`, `term.attach` and source-asserted `term.close`
accept the same exact original-source selector under `term.projectSources`
(wire 3.21.0, #531). Plain folders support shells too. Terminal scope parsing
in `app/runtime/terminals/scope.rs` rejects malformed, foreign, mixed selectors
and caller roots before opening a PTY; `source_path` asserts the registered
location, and a moved location returns `conflict`. Each explicit source shell
keeps immutable project/source/location ownership beside its canonical cwd.
Lists and `terminals` facts use that ownership, so registered aliases sharing
a folder still have separate explicit tab rows. Legacy scopes remain canonical
root aliases, including shells opened before an explicit source selector was
available; input/resize/ack continue to address the captured terminal ID.

Client detach or reconnect keeps the shell and ID. Explicit tab close, EOF,
a missing root and project deletion use the existing kill/reap and screen
closure lifecycle. Source move/remove refuses while shells stand in that
source folder, naming their IDs. Create and move/remove admission run under
the app lock; physical scope probes and PTY spawn run outside it. A pending
spawn holds its checkout and source location until registration or child
retirement. The existing Updating reservation blocks shell starts and source
removal throughout deferred source update work and settlement. Reserve and
commit guards recheck project deletion; deletion refuses while that project's
shell spawn is pending and collects its registered shells for retirement,
even when their original source folders survive deletion.
Label changes leave live shells alone. These holds
keep the source's membership/location consistent; they do not restrict shell
commands or serialize them with Git/base sync. Worker holds also permit
source shells and base sync; both shell and worker holds must end before
relocation or removal.
A worker-held source can open and attach a shell while base sync waits on its
Git lock, and sync can fast-forward with that shell still open. Closing the
shell leaves the worker's location hold intact. An already-admitted terminal
create rechecks Updating before reservation and registration: a move's
reservation holds it back during the SyncLock wait and after work completes, until config settlement. The old
selector then conflicts; a fresh selector opens at the settled location.

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

Build-managed Git mutations across all resolved scopes (including legacy
first-source verbs and adopted workspace aliases),
source settings edits and base sync share `source_sync::SyncLock`, keyed by
canonical common Git directory (#530). Linked worktrees share that lock;
independent workspace repositories have their own. They acquire it off the app mutex and
revalidate source membership/location after waiting. A settings edit locks
its old/new repositories and affected independent workspace repositories in
canonical order, deduplicating aliases, through config settlement. Reads and
unrelated RPCs continue while an operation waits. Shell Git and file edits
remain outside this lock.

Worker holds pin source locations without preventing Git or base sync. Admitted
source moves retain their counted worker-start hold across repository-lock waits;
settlement releases only that move's count on success or failure, while retaining
its Git locks through source-record settlement.

Cut preparation groups aliases by common Git directory and pins exact configured
base OIDs under that lock, including when automatic sync is disabled. It keeps
#271's fetched OID when safe; otherwise it uses the configured base's committed
state, including local commits, never another checked-out branch's HEAD. Cut
warnings disclose local commits and a stale remote separately. Materialization
uses the pinned OID after releasing the lock and retains existing worktree/Rift
copy rules. Adding a local Git directory to an existing workspace also pins its
base before materialization. A lock wait beyond the cut's ten-second deadline
refuses creation rather than resolving an unprotected mutable base ref; a fetch
failure still cuts the pinned local base with its existing warning. Contention
does not prove a fetch occurred: a requested fetch uses only the budget remaining
after the lock wait. Refused preparation removes only that create request's
unpublished root and preserves existing workspace files.

Retry preparation uses only unfinished directories in the saved workspace
manifest. Their saved source identity, path and base remain the inputs after a
project source moves, is removed or changes base; those changes alone do not
refuse retry. A matching current source supplies its sync policy, while a
detached saved input is pinned locally without automatic sync. Every Git
directory requires an exact prepared OID before materialization. Failure to
access or resolve a saved input preserves the published retry root and all
ready directories. Settling an old cut never requests sync for a moved or
rebased replacement source.

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

**Workspace locks** (#398). `workspace.set_locked` takes `workspace_id` and
the Boolean `locked`, persists the change, returns a `WorkspaceRow`, and notes
the workspace list changed. Managed manifests retain the flag; older manifests
default to unlocked. Adopted checkouts keep their locks in the registry's
`.locks/<workspace id>` metadata, so Build does not write into their working
copies. The command refuses `busy` while deferred filesystem work or a reclaim
reservation could overwrite the record or has already admitted a removal.
The browser calls this RPC from the toolbar control beside the workspace name
(`spa/src/core/toolbar.js`, `spa/src/styles/workspace-lock.css`), drawing from
the cached workspace row. Agents can read the flag but have no MCP action to
change it.

The bridge checks the lock before workspace Delete, Done and explicit reclaim,
before deleting a project with a locked workspace, and before run cleanup that
would remove files inside one. These paths return the same nonretryable
`locked` refusal before stopping processes or changing files. The SPA disables
the corresponding deletion controls while their cached workspace is locked.
The reclaim service also records `locked` as a hold, preserving build output.

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
sweep, which the change asks for at once. What holds it: locked, not ready, an agent working or a
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

Each project agent gets one notice per sweep naming its quiet workspaces
whose notices are due. Unlocked workspaces repeat once per idle period
(daily by default); activity after a notice resets that cadence. Locked
workspaces repeat only after seven days since their last notice, even if
activity happened in between, and are omitted from intermediate notices about
other workspaces. The existing per-workspace `noticed_at_ms` persists that
clock across restarts. Every notice tells the agent to message the user only
if necessary and otherwise merge, reclaim or leave the workspace without
posting to the user. A locked workspace was locked on purpose: the agent
evaluates whether a reminder is needed, without reclaiming it or asking to
unlock it by default. The linked tasks record `workspace_idle` and
`workspace_pruned` as actor `build`, without waking their trackers.

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

Task prerequisite storage (#560) lives in `store/waiting_on/`, separate from
parent links. Schema 16 adds the normalized directed graph, readiness state,
durable reconciliation cursors, transition receipts and task invalidations.
Edges are same-project, bounded at 20, and checked for cycles under the store's
IMMEDIATE write transaction, including edges already resolved by Done or close.
Forward summaries are batched; reverse reads seek by immutable task number
with a project/blocker/store-bound cursor.

The common tracker writer journals status, state and title changes, so ordinary
updates, review lifecycle writes and workspace finish all reach the same
reconciliation rule. `app/tracker/waiting_on.rs` drains bounded pages at startup
and after committed writes, off tokio workers, and invalidates affected task
summaries through the existing change bus. Readiness events preserve Done/close
provenance and have durable outbox receipts. This storage slice grants no
assignment delivery authority; durable assignments are the following #561
slice. #562 verifies the graph, dispatch and read contract and announces
`tasks.waitingOn` in the still-unreleased wire 3.21.0.

PR-style opening is a service in `bridge/src/reviews/opening.rs`, exposed by
wire 3.15.0's typed RPC and MCP adapters (#405). `api/v1/reviews.rs` owns the
strict wire selections; `app/tracker/reviews/` resolves project, workspace,
source and caller identities under the app mutex and drains Git work off it.
Clients name stable IDs and branches, never repository paths or actors. The
bridge attributes RPC calls to the user and MCP calls to the authenticated
agent in that project.

Review opening and the explicit lifecycle jobs load their workspace review
observations from SQLite off the app mutex, after durable publication.
Invalidation installs the owned observation before publishing its workspace
summary and replying. Close commits off the mutex and captures its completed
task and events in that same transaction. Apply installs the review summary
before publishing the task and delivering its completion notices once;
immediate workspace reads after RPC or MCP close see the closed review.
Merge progress and background review sync load observations
off the mutex at their existing publication checkpoints, then take the mutex
to install them. A monotonic observation generation prevents a delayed load
from replacing a newer lifecycle observation, even when another review task
now owns the workspace; installation also rejects a removed workspace.
A failed observation load preserves the accepted summary for maintenance to
refresh. Periodic maintenance captures the workspace IDs and an owned review
reader in its existing capture section, loads review summaries alongside the
off-lock project refresh, and installs them through the same generation fence
in its existing apply section. Physical workspace refreshes do not query review
SQL; startup warms review summaries before shared admission.
Once opening has published durably, an observation failure does not turn it
into an error or skip settlement. Close likewise returns its captured completed
review if the subsequent optional sync/merge-intent projection fails, and still
settles its captured task and completion notices once.

| RPC verb | MCP tool | Contract |
| --- | --- | --- |
| `tasks.review.open` | `open_review` | Workspace, request ID, title/description, optional reviewer, per-directory bases and excluded Git IDs. Returns the published task/review, reviewer delivery state and explicit local push instructions. |
| `tasks.review.push` | `push_review` | Review version and selected directory/head OIDs; explicit force-with-lease also names the expected received head. Returns independent per-directory publication outcomes. |
| `tasks.review.update` | `update_review_base` | Review version and per-directory base branches. Publishes a fresh snapshot against the updated targets. |
| `tasks.review.merge` | `merge_review` | Review version, latest snapshot, every included directory's expected target head and optional remote/branch publication. Uses the durable merge service. |
| `tasks.review.close` | `close_review` | Review version and a short action description. Closes without claiming a merge. |
| `tasks.review.reopen` | `reopen_review` | Review version. Reopens a Closed PR only when its original workspace and receiving setup remain recoverable. |
| `tasks.review.refresh` | `refresh_review` | Review version. Explicitly reconciles registered receivers, including observations on retained terminal PRs. |

Review results preserve `{review}` and add `sync` observations and
`merge_intents` only when populated. Push additionally returns `sources` and
optional recovery guidance. Task get/list and watched-task feed rows carry an
optional `review_summary`; workspace get/list carry optional `active_review`,
including the latest retained terminal link when no active PR remains. No new
push event or subscription kind is introduced: existing task and workspace
invalidations make clients refetch. `tasks.review.pullRequests` and each verb have
separate cached SPA capability flags; no flag is inferred from the wire minor.

Each sync observation may also carry `reviewed_snapshot_id`, the snapshot the
user last gave an opinion on, with `commits_since_review` (commits reachable
from `received_head` and not from that snapshot's head) or
`rewritten_since_review: true` when that head is no longer an ancestor (#427).
The bridge counts in the receiving repository, where snapshot pins keep a
rewritten head readable, with a port of libgit2's `graph_ahead_behind`
(its paint, stop rule and priority-queue tie order) using libgit2's
timestamp-only ordering: commit-graph generation numbers are not used (#462),
and the count can differ from `git rev-list`, for example under clock skew. It
is capped at 2000 commit lookups, wide merges' parents included
(`reviews/sync/since_review.rs`); past the cap both fields stay absent. A
stored result for the same `received_head` and baseline is reused, so idle
polls walk no history. The fields are
optional additions to 3.15.0 under `tasks.review.pullRequests`; agents'
opinions never set the baseline. A new opinion does not trigger a
reconciliation, so the next event or 30 s poll refreshes the count.

Refusals use existing error codes with structured details such as the task,
directory, expected/current version, reason and recovery action. Partial Git
results remain successful reply data, so one failed source does not hide another
source's saved success. Recovery and publication retries require explicit calls.

The opening service uses schema 15's journal to reserve a task identity by project and
request ID, prepare dedicated branches in a managed workspace, then publish
the task, PR header and first snapshot in one store transaction. Failed or
interrupted preparations retain their recovery claim; retry resumes the same
identity, and explicit cancellation removes only owned setup at expected
values. The original workspace branches and dirty index/files are preserved.
Cancellation checks all original branches for checkout, rebase and bisect
holders and validates owned receiver and tracking refs before removing setup.
It repeats the original-branch holder check before restoring HEAD.
`receivers.rs` owns a local bare repository per source repository outside the
workspace, and `publication.rs` imports and pins explicit committed OIDs there.
Initial publication fetches objects without updating refs, then creates the
receiving branch through the registered writer instead of native receive-pack.
It also establishes the working repository's owned remote-tracking ref before
success, including retries after the receiving write, so UI Push uses the local
review remote.
Git ref and config writes use registered Git locks that can
recover a dead owner's exact inode while preserving replacement locks. A new
receiver is initialized in a separate staging directory and published only
after its files are durable. Interrupted staging is retained without blocking
retry. The caller holds the workspace mutation lease and supplies hooks for
live reclaim/placement
checks, manifest installation and idempotent reviewer dispatch. Reviewer
delivery runs after publication, with a durable status and explicit retry.

New receivers share immutable source object files through hardlinks when the
filesystem supports them (#839). They have their own directory entries, so
source ref deletion, garbage collection and source removal cannot invalidate
published reviews. Index files have independent inodes; linked packs have
receiver-local `.keep` markers to avoid repacking another physical copy.
Receivers never use object alternates: native
Git pushes cannot atomically install protecting refs in the source repository.
Optional seeding runs on a dedicated niced thread with a five-second budget and
verifies the imported graph before publication. Unsupported links, incomplete
graphs and budget exhaustion fall back to importing the selected commits and
their reachable history. Garbage collection at creation would not remove that
reachable history.

The daemon registers `reviews/sync.rs` independently of client subscriptions.
Its blocking worker discovers active PRs in pages, watches canonical worktree
Git directories, shared common directories and bare receivers, and reconciles
at startup and every 30 seconds. Metadata callbacks enqueue identities only;
tasks coalesce for 500 ms of quiet with a two-second maximum and retry with
bounded backoff. Its store loader reads only bindings and the latest published
snapshot; retained history and actions add no background polling reads.
Metadata watches are deduplicated across linked worktrees and recurse only
through branch refs. Objects, working-file writes, access events and private
pin trees neither allocate recursive watches nor trigger publication. Missing
watches are covered by polling.
Discovery, repository resolution and watch registration share an eight-operation
budget per worker turn; registration has no whole-set tail at the end of a scan.
Polling preserves a task's existing quiet deadline. Client worktree watchers
also share canonical Git metadata coverage and exclude private pin/ref-log
trees before registering kernel watches.

Reconciliation reads only each registered receiving ref for publication. It
compares the fixed directory/head/merge-base vector with the latest snapshot,
then pins explicit head, comparison base and selected target tip in the owned
receiver. Source target movement is recorded separately in sync observations;
only comparison-base movement creates a `base_changed` snapshot. Terminal
pushes use actor Build, and non-fast-forward received heads record rewritten
history while retaining old snapshots and their snapshot-scoped opinions.
Receiver ref locks and the review version check fence publication. A persisted
snapshot atomically updates bindings, resets PR status to Open and moves the
task to In review. Observations have separate, deduplicated revisions, and each
persisted result invalidates the task and workspace. Deleted receiving refs or
unavailable required bases retain the previous snapshot and expose sync health.
Snapshot readers validate the recorded receiver without requiring the original
source or disposable workspace.
Before capture, sync records one pending candidate per task in SQLite and writes
a durable candidate journal in the registered receiver. An advisory lock
excludes another sync writer through capture and publication. Explicit
reconciliation waits up to two seconds for an in-flight capture, then reloads
the current review under that lease. The background worker acquires immediately
or schedules its existing retry, so one busy task does not stall its turn.
Capture completion explicitly unlocks the directory, including error paths;
an unrelated child's inherited descriptor cannot extend the completed capture.
Restart and poll discovery include pending candidates even after a PR closes:
published candidates retain their pins, while unpublished candidates use the
existing marker/OID-checked cleanup. Recovery reads one candidate and an indexed snapshot
existence check, without scanning snapshot history. Project history deletion
waits for outstanding candidates to recover.

PR lifecycle writes live in `reviews/lifecycle.rs` and
`store/reviews/lifecycle.rs`. Only each actor's latest opinion on the current
published snapshot contributes: any request for changes gives ChangesRequested,
otherwise an approval gives Approved. Older opinions stay in the timeline.
Open and Approved map to In review, ChangesRequested to In progress, and Merged
and Closed to Done. Review status, task movement and their activity commit
together with a review version check. Ordinary board moves remain organization;
moving to Done or completing an active PR closes it without claiming a merge.
Tracker closure also closes an active PR and moves it to Done. Both terminal
statuses retain the older review state's Completed compatibility. Ordinary
comments and read-time identity/completion backfills preserve concurrent
lifecycle writes, including Done and archive timestamps.

Only a Closed, unmerged PR can reopen through the lifecycle service. It requires
the original managed workspace, configured sources, dedicated branches,
registered receivers and owned push routing to remain recoverable, with no
competing workspace claim. Successful reopen publishes a fresh snapshot even
when the received heads are unchanged, resets opinions and restores In review.
It preserves the user's workspace lock. A Merged PR needs a new PR for further
review. Explicit reconciliation of a terminal PR updates bounded observations
of later received work or unavailable receivers without changing its status,
version or snapshots; ordinary worker discovery still visits active reviews
and pending captures rather than polling retained terminal history.

Receiver maintenance (#839) runs independently on a niced background thread,
without the app mutex or an RPC caller. It releases merged reviews and Closed
reviews whose original managed workspace is no longer retained, including their
snapshot pins and opening journals; an unused owned receiver is removed. Closed,
unmerged reviews retain their receiving setup while the original workspace stays
ready, so reopen still requires that workspace. Metadata and comments survive
receiver reclamation; historical Git content can become unavailable. Startup
also collects retained receivers, with failures retried by later turns and
successful collections throttled to once an hour. Build-owned pack keep markers
are released when the source no longer shares their inode.
Running or unsettled merge publication retains its receiver until saved retry
work settles, even when local integration already marked the PR Merged.
Active receivers use Git's normal pruning grace period: `--prune=now` could race
native pushes before ref publication. Maintenance reports reclaimed bytes and
preserves receivers whose ownership or in-flight operation cannot be established.
Its Git-lock recovery uses a maintenance-only try-lock policy and retries contention;
foreground opening and recovery retain their existing wait semantics. Recovery,
opening and PR fence lock guards explicitly unlock on scope exit, including
errors: CLOEXEC alone cannot release a flock inherited by a child before exec.
The recovery guard Drop, previously reverted for #845 scope, is required by
this production concurrency fix; no lock wait deadline changes. Contended
maintenance lock destruction records exact released ownership for a later reaper,
so a live daemon PID does not strand its completed reservations.
Unused receivers first close native Git admission with owned
`reference-transaction` (prepared-state) and `pre-receive` hooks in the gitdir's
own `hooks/` directory, with `core.hooksPath` unset so discovery follows a
quarantine rename. Foreign hook overrides are preserved. The marker and active
hook configuration are rechecked inside quarantine before the final inventory.
Migrating legacy absolute hooks restarts the full one-hour drain; inherited-hook
refusals preserve the original config bytes. A durable close
marker starts a one-hour admission drain; later turns require both that drain
and an empty gitdir/ref lock tree before quarantine. Git's normal pruning grace
remains independent and applies to GC of retained receivers. A new bridge
opening removes that marker. Complete ref and object inventories are checked
after quarantine; late or foreign state and unapproved interrupted quarantines
are preserved. Only the private quarantined path is deleted. A reappeared
canonical path is reported and retained. Deletion journals use a separate stable
lease inode so atomic journal replacement cannot admit another cleaner.
Accepted residual risk (#839): a Git process started before the close marker
can retain a cached absolute gitdir, outlive the grace period and lock drain,
and write afterwards. Receivers are bridge-internal; deletion is limited to
terminal reviews with no retained reopenable workspace or unsettled work.

`reviews/merge.rs` owns durable PR merge intents, independent of a browser.
Admission reconciles receiving refs, checks the selected latest snapshot and
version, and records every included Git binding with its configured base and
expected target tip. A repeated project/request ID retains the same plan;
changing that plan is a conflict. Source identities and checkout placement are
rechecked under source-sync sequencing. Per-step input and results commit before
the next Git step starts. Multiple directories sharing a repository/base advance
the expected tip in order; a partial failure retains successes in the other
repositories. A ref-transaction fence rejects target movement between the
preflight read and Git's actual update. The per-task merge lease is explicitly
unlocked when the operation returns, so a retry or recovery cannot mistake an
unrelated child's inherited descriptor for an executing merge.

PR merges resolve effective hooks from the actual target checkout before
applying the per-command ref fence. Traditional default/configured hook paths
and Git-configured hook commands retain their arguments, stdin, enabled flags
and vetoes; the original reference-transaction hook receives its transaction
phases and bytes while the prepared-phase expected-OID guard remains active.
Delegates restore the original `GIT_CONFIG_PARAMETERS` so nested Git commands
retain the user's hook routing. Repository hook files and configuration stay
untouched. The fence directory contains hook resources only, registered under
the canonical common Git directory with pinned ownership markers/files,
directory leases and process/group identity records. Startup visits persisted
action source paths, including settled results, before merge recovery and removes
only registered resources whose ownership and inactive process groups are
verified. Live or unverified launches, changed ownership and old unmarked
temporary directories are preserved.

Finalization reacquires native receiving and target ref locks, verifies each
reviewed head is still received and contained in its configured base, and checks
the current snapshot, review version and intent-owned successful action results
in the metadata transaction. Only that proof sets Merged and Done. Target-base
movement caused by the admitted merge is deferred while its current-snapshot
integration remains unsettled; newer received heads still publish immediately
and prevent stale finalization. Optional external publication can fail after
local integration: the PR stays Merged with its failed or interrupted Push
result visible. Explicit retry publishes the recorded successful merge tip,
never reruns that Merge, and can settle saved publication after a newer snapshot
or closure without changing the newer review lifecycle. Startup recovery never
replays Git; it finalizes recorded complete integration or preserves incomplete
or uncertain work for explicit retry, excluding live merge workers.

Explicit merge retries keep the saved completed or partially integrated plan
and its recorded results. RPC selection first matches the exact saved source
vector, including target preconditions, snapshot and Push destinations. Only
when no exact match exists does it match refreshed targets for the same snapshot,
included directories and Push destinations. Selection is independent of the
retrying actor, so a browser can resume an agent's intent, including older
actor-hashed request IDs. Among equally exact vectors, intents with unsettled
publication or running or uncertain Git work rank before settled intents. A
previously failed intent whose publication has settled cannot hide another
retained hold. A historical Interrupted Push stops ranking as pending only
after a later success in that intent matches its source, snapshot, input head,
destination and original Merge linkage. Its interrupted history row remains;
earlier or foreign results do not settle it. Running and other unresolved work
keep their recovery identity.

For a partial plan with known outcomes, explicit confirmation of the current
version and target tips refreshes admission under the merge lease and receiving
and target ref locks. Only unresolved target preconditions and admission versions change;
the request ID, original actor, saved heads, source identities, Push destinations
and action results remain. Successful sources must remain integrated, and Git
fences each remaining merge against its confirmed tip. Running or uncertain work
cannot be refreshed. The SPA offers a separate preparation step with fixed Push
choices; preparing the form never dispatches Git. Submission consumes prepared
mode and durably saves the confirmed targets. Both saved retry controls retain
those targets across reloads for the active latest plan; another explicit
preparation permits new targets. Publication-only result retries use the intent's
saved vector to address its original obligations.
Each retry reads and saves the draft for the intent's own snapshot, preserving
that snapshot's confirmed targets and choices. The clicked review version and
plan remain fixed while the draft loads; a newer cached opinion or snapshot
cannot silently advance the request's confirmation. Retrying another snapshot never
changes the selected form's draft. The form accepts only submitted requests for
its selected snapshot and ignores foreign submitted requests left by older
clients; preparation also requires that the intent's snapshot is selected.
Historical plans retain their publication-only recovery path without admitting
a new merge. If a failed plan integrated no source and has no uncertain Git work,
the caller can still confirm
the current review version to admit a fresh plan after the review changes.

Reclaim holds a workspace while a durable merge is running or publication of a
recorded successful merge tip remains unsettled, even after the PR reaches Done
and after restart. Unreadable publication state also holds the workspace and
its build output. These checks run during measurement and again before removal.
Settling publication releases only that hold; workspace locks and ordinary
activity, task and Git eligibility checks still apply. Project history deletion
refuses running merges before releasing any review pins.
The SPA evaluates those retained results from cache: a later intent-owned Push
with matching source, snapshot, tip, destination and Merge linkage settles an
Interrupted Push without erasing its history. Known failed sources from that
intent do not inherit the settled interruption. Running or unknown Git results
and missing action records still prevent Reclaim; cached workspace locks continue
to control the enabled state after publication settles.

`bridge/src/reviews/` saves one task's workspace review as numbered snapshots.
`capture.rs` reads every manifest directory, resolves each Git directory's
committed HEAD and base (explicit override, configured base, upstream, empty
tree), and pins its nonempty base and head under private refs. Ref components
percent-encode bytes outside ASCII letters, digits, `_` and `-`: directory IDs
contain `:`, which Git cannot use in ref names. Metadata keeps the original IDs.
The snapshot ID is unique before any pins are written; `service.rs` saves the
metadata with a review version check and removes only the losing call's pins
at their expected OIDs. For Snapshot-mode reviews, a snapshot from another
workspace replaces the previous snapshot history; its old pins are released
after the metadata commits. Git
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
description limit. Snapshot-mode task closure and workspace Finish remain
separate. Ordinary moves to Done complete an open review through the same writer
with “Marked done”; for PR mode this records Closed. Snapshot-mode captures
change only review metadata. Agent Complete reports and merged
workspace Finish skip their old task-movement/closure hooks for review tasks.
Closing/completing and removing workspaces or projects retain review metadata.
Snapshot-mode refs remain until workspace replacement or explicit task history
deletion; PR receiver refs follow the maintenance retention rules above. MCP exposes
`snapshot_review`, `get_review`, `read_review`, `act_review`, `complete_review` to coding and
project agents, authenticated and restricted to their own project.

`reviews/actions.rs` executes Snapshot-mode selected source steps off the app
mutex and publishes task invalidations after each persisted result. Targets resolve from
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
Legacy `tasks.review.act` and `act_review` refuse PR-mode reviews before action
admission; the RPC returns nonretryable `invalid_params`. PR actions use the
dedicated merge lifecycle service. Existing Snapshot-mode Merge and Push
behavior is preserved.

The task page embeds the saved review (`spa/src/core/taskReviewPage.js`), with
every saved directory and Changes/Files views. The Workspace Changes header
and workspace-name menu open one shared PR creation form
(`workspaceReviewEntry.js`, `workspaceReviewForm.js`). It selects a reviewer
and per-directory bases, previews dedicated branches, and retains the exact
submitted opening in F's durable create draft for explicit recovery. The form
returns a definitively rejected opening to an editable durable draft; uncertain
outcomes retain the submitted request. Titles are checked against the bridge's
200-byte UTF-8 limit before submission. Draft cleanup only changes the matching
submitted value and write stamp, preserving a newer draft from another tab.
The header paints the bound PR, snapshots and publication observations from
cache. Push
confirms the destination and saved review version, working head and received
head through F's action draft; a stale request stays pinned until the user
explicitly selects the latest cached changes. Another branch or a detached
checkout offers switching back rather than publishing that checkout.
Reviewed-snapshot notes say changed or rewritten, or remain absent when
unchanged; an exact commits-since-review count ("2 commits since your last
review") paints only from a cached observation whose `reviewed_snapshot_id`
names the same snapshot as the cached timeline's last user opinion and whose
`snapshot_head` is the displayed snapshot's head, and an older
bridge's observation leaves the qualitative note. PR task controls show read-only status while lifecycle UI is
pending. Older bridges and pure-folder workspaces retain the legacy snapshot
form. Legacy snapshot/base changes and explicit completion live in
`taskReviewControls.js`; assigning a reviewer uses the
ordinary assignee picker and a note naming the snapshot. Opinions and line
comments use `taskReviewFeedback.js` and stay in the task timeline, with no
reviewer or model restrictions. For legacy reviews, `taskReviewActions.js` offers
separate Merge and Push selections for each source, displays destinations and saved/live heads,
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

Codex app-server provider errors are turn failures, so an exhausted
`error` notification (`willRetry: false`) keeps the process and provider thread
available for the next message. Build classifies `codexErrorInfo`, its upstream
HTTP status, and explicit timeout/overload text (`codex_app_server/errors.rs`).
Stream disconnections, HTTP 408/5xx and internal-server failures receive one
continuation after `PROVIDER_RETRY_BACKOFF` (60 seconds). The continuation names
the sanitized error and asks the agent to continue from its preserved context;
it keeps the failed turn's model and effort. Codex's own `willRetry: true`
notifications consume no Build retry. Auth, quota/429, invalid requests,
context-window, sandbox and unknown failures never retry automatically. Permanent
failure hints in either the message or additional details veto a transient
stream wrapper; the unclassified `other` variant never opts into text fallback.

The session remains working during the backoff. `provider_recovery.rs` records
the failure episode, failed operation, absolute deadline and recovery stage in
a synchronous sidecar before the backoff and before sending its one
continuation. A bridge restart abandons a checkpointed recovery and reports its
failed dispatch, rather than starting an ordinary resume turn with a fresh retry
allowance.
The failure remains pending until its notices are saved; repeated boots reuse
the same episode identity. Delayed startup replay leaves an episode owned by
a live session alone; the continuation barrier rejects an episode already
abandoned or settled. Queued input does not clear the checkpoint. A successful
retry resolves it before starting queued work, while an exhausted retry keeps
its failure outbox. A new turn can start a fresh episode afterwards. A saved
dispatcher notice retains its recipient generation and message identity, so
boot can requeue its wake while the notice remains unread without posting it again.
An unrecovered turn failure or an unexpected protocol transition that ends the
session publishes one
`TurnFailed` activity. Terminal status becomes observable only after its failure
checkpoint and activity reports are ready. If the checkpoint write fails, the
activity uses the direct dispatcher failure-notice path; structured diagnostics
retain the internal I/O reason without provider-text sanitization. The final
status watch precedes activity-stream closure, and direct status reads wait for
that closure. The app records a failed outcome, delivers a failure
message to the dispatch requester (falling back to the project's coordinator),
and comments once on the dispatched task. The activity carries the failed
operation so a newer consumed turn keeps its task and execution clock. Task
comments require current assignment and conversation authority; ordinary
completion reports retain their existing semantics. Failure notices use the
ordinary 30-second notice settle window and wake without a client connected.
The failed holder is not woken to read its own failure. Lifecycle telemetry
records the allowlisted error code and `codexErrorInfo` variant name, numeric
HTTP status, and a bounded human message or epitaph. Only the text before the
first `{`, `[`, `<`, `=`, or `:` is retained; this also excludes unlabelled
bodies and plain key/value dumps. Credential-shaped values in that prefix are
redacted, the discarded suffix receives a fixed redaction marker, and the
result is bounded to 1 KiB on a UTF-8 boundary. The raw error still drives
classification; discarded text never enters telemetry or failure notices.
Build-generated protocol failures use a separate enum-based diagnostic path:
`protocol_failure` retains only fixed event, phase and connection-error labels.
The resulting summary preserves the protocol cause in terminal telemetry,
`TurnFailed` and its durable notice, without copying provider IDs or free-form
error details. These trusted summaries do not pass through the provider-text
delimiter sanitizer.
The terminal snapshot claims the summary and its typed provenance together
under one lock, then freezes that cause before publication. Telemetry and the
durable notice use the captured cause; a later failure cannot relabel it.
An exhausted provider notification logs `terminal_error` with `scope: turn`;
it does not request process shutdown.

Capture routers own no canonical conversation or session lineage, so their
startup omits conversation recovery context. Delivery accepts their missing
session instance only after validating the registered router target and tab
identity. Ordinary agents retain conversation recovery and exact session
instance fencing.

ADK reports `Working` while a parent turn is open or its background-task roster
is nonempty. A delivered turn opens when its final newline is written. The
child can also start a turn itself: top-level assistant text, thinking or tool
calls, and top-level user tool results open it until the next `result`. Nested
subagent activity, echoed prompts and passive task/control events do not open
a parent turn. Both starts share interrupt and steering semantics; an
interrupted result hands the open turn to any queued steering message. The
reader publishes `Working` before success or activity updates, so the registry
starts the working clock and the idle sweep protects the resumed turn.

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
- Pi is asked `pi --version` (with `PI_OFFLINE=1`, so its startup checks
  nothing online) for its version alone, and offers its catalog whole.

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
`MISE_OFFLINE=1` (and `PI_OFFLINE=1`), so a mise wrapper never starts an install that the deadline
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

**The harness inventory** (#434) is `bridge/src/harness/inventory/`, a daemon
service with its own lock (never `AppState`'s). It keeps a revisioned snapshot
of all five harnesses, each linked to a credential context it shares with the
other carrier of its CLI (`Harness::auth`): `claude` (Claude Code TUI and
headless), `codex` (TUI and app server) and `pi`. Each row carries three facts
apart: installation (`installed` / `not_installed` / `unknown`), the installed
CLI's version from the readings above, and the versions live sessions
reported (`inventory/running.rs`: the ADK init line and the app server's
`initialize` hold a `ReportedVersion` for the session's life; the TUIs report
none, so theirs reads "not reported" rather than borrowing the installed one).
Each context carries `method` (`oauth`, `long_lived_token`, `api_key`,
`external`, `mixed`, `none`, `unknown`), `status` (`signed_in`,
`not_signed_in`, `expired`, `invalid`, `refresh_pending`, `unknown`,
`not_required`), `verification` (always `saved_configuration` once observed:
nothing here is a provider check), the kinds of `evidence` it was read from,
`health`, `checked_at_ms`, a `credential_generation` that rises when the
credential changes, and `supported_login_methods` (empty until in-app sign-in
lands). Pi's context lists each provider on its own.

The adapters are passive and metadata-only (`inventory/adapters/`): no
authentication read starts a process (installed versions still come from the
CLI readings above and their own sanitized probes). Claude Code's `auth status` is not passive through its
startup (2.1.284's root pre-action hook can refresh OAuth and run policy
helpers, #466), Codex's `login status` prints part of an API key and its app
server's account read could refresh, and Pi has no status verb. Claude
Code's adapter reads `expiresAt` and which tokens `.credentials.json` holds
(under `CLAUDE_CONFIG_DIR`), `primaryApiKey` in the global config (an API key
saved by `/login`), an `apiKeyHelper` in any settings source a
session started in the home directory reads (user, the home's own project
and local `.claude` settings, managed policy and its `managed-settings.d/`
drop-ins), and token, key and cloud-provider variables by name. Codex's reads
`auth.json` and `config.toml` (parsed as TOML) under `CODEX_HOME`. Pi's reads
`auth.json` under `PI_CODING_AGENT_DIR` and its documented provider variables,
and never resolves a key (a `!command` key reads as `external`). Every file is
opened without blocking, refused unless regular, and read up to 1 MiB; a
missing file means nothing is saved, and one that is there but unreadable or
malformed fails the observation, so the prior facts stand, stale. One
presence rule holds for every credential field and variable
(`adapters::is_blank`): a credential is there only as a string holding
something other than Unicode whitespace (`str::trim`); `null`, booleans,
numbers, arrays and objects are absent. Codex's externally managed token
mode counts as signed in only with a token. Claude Code's global config is
resolved in its own order: `<config dir>/.config.json` when it exists, else
`.claude.json` in `CLAUDE_CONFIG_DIR` or the home directory.

The service sweeps every 15 s from daemon start with no client needed
(`inventory::start` in `main.rs`): executables each sweep (installs, removals
and retargeted links are seen at once, and so does `models.list`'s
`installed`, no longer cached for the process's life), each context at most
once a minute after success, backing off from a minute to ten after failures.
A changed executable, credential or settings file, or `harnesses.refresh`, checks again
sooner, no more often than every 5 s. A failed check keeps the facts it could
not replace with `health: stale`; an older check never overwrites a newer one.
Each change raises the revision, is saved nonsecret to
`harness-inventory.json` beside the store (restored stale on boot, the
revision continuing), and wakes `harnesses.changed` (`{type, revision}`) for
each greeted session (`bridge/src/app/harness_inventory.rs`). A check time
moving alone is not news. `harnesses.list` answers the snapshot without
waiting; `harnesses.refresh` answers `{request, revision}` at once, and the
snapshot's `refresh.completed` reaches `request` when that observation is in.
The SPA routes the push per device whether or not change subscriptions are
armed (`spa/src/core/harnessInventoryEvents.js`). Controls and evidence:
`planning/v2/Harness Authentication Security Checklist.md`.

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
MCP admission captures the source thread generation. Done reports and agent
writes validate it under the mutation lock. Independent read capabilities also
retain the issued source generation and validate it against saved addressing
and private runtime authority. The runtime reset
reservation remains active through complete commit or failure restoration.
Saved addressing, thread admission and task overlays publish every affected
alias in one metadata batch. Source/roster guards consult the reservation;
router history targets may read the complete old committed history during
retirement, with their frozen binding/thread fence rejecting a replacement at
completion. Deferred task handoffs retain the source generation until
settlement. A late callback or a request carrying the old generation cannot
restore the cleared content.
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

The stdio adapter's `mcp/client.rs` sends an action once and keeps its connection
and partial response through socket read timeouts. An absolute five-minute reply
budget bounds the wait; reaching it reports that the call may have been applied
and must be checked before retrying. It never automatically resends an action.
Pi's owned stdio wrapper allows 310 seconds for `tools/call`, so the Rust
adapter's five-minute result can arrive first. It queues tool calls locally and
starts each deadline when that call is dispatched, after the previous call's
reply; time waiting behind another call does not consume the active-call budget.
Its existing 30-second initialization and tool-discovery deadlines remain
unchanged. If the outer
tool-call deadline fires, it also warns to check the outcome before retrying.
An authenticated action the daemon cannot parse gets an explicit error response,
so a newer adapter's unsupported action does not wait for the reply deadline.
MCP control-frame slow logs account for semaphore admission (`permit_wait`) and
blocking pool scheduling (`pool_wait`). These fields appear only when measured;
ordinary RPC frames omit them. Both MCP and ordinary RPC dispatch measure every
app-lock acquisition (`lock_wait`), including the update-admission check.
Reacquiring a turn after deferred work is included in the permit wait; the
deferred work itself is not pool queue time.

The two control turns continue to bound mutation work. MCP task, roster, list,
history and review reads on an exact allowlist use a service captured during
listener setup and take neither a writer turn nor the app mutex. Malformed
actions and done reports retain their existing routing. Private immutable
capability state publishes token installation, rotation, revocation and router
incarnations immediately. Token bytes authenticate before issuer data is used;
coding capabilities bind to the validated issued owner, agent, canonical
conversation and thread generation. Project scope and tool surface follow
current committed policy rather than becoming lifetime token entitlements.

Saved immutable addressing retains roster order, canonical bindings, thread
identity and persisted pin/name policy. Failed saves cannot admit candidate
identities; temporarily borrowed owners retain their saved records. Roster and
list reads use accepted producer fragments and scope/selection metadata, with
roster target stamps taken from the captured payload itself. Read preparation and final
validation compare those targets with saved addresses and current authority;
changed bindings, generations, selections or project paths refuse the captured
request instead of retargeting it. No request initializes producers, probes
workspace paths or drains deferred writer work.

SQL reads use the shared asynchronous reader pool and one transaction across
related rows. The pool shares quotas across listeners and service clones:
three in-flight MCP task-list requests and two per other MCP SQL read type.
A caller acquires its type's permit before borrowing a connection, so excess
callers wait outside the eight-reader pool. Three task pages, two task details
and two history reads occupy at most seven connections, leaving one reader
for other read types; a fourth task page waits before SQL checkout. Both the
connection and type permit are released before external I/O; their acquisition
and work take no app lock.
Snapshot materialization and the exact harness catalogue resolver
run off-lock. The catalogue combines accepted default/ordered-role settings
with the existing CLI readings service, preserving TTL/executable refresh and
hidden-model startability without a new catalogue cache or publisher-side
executable probes. Review work releases SQL capacity before external I/O and
retains its observation fences. Final checks reload immutable capability,
address and accepted scope metadata; comment-name decoration uses saved policy.
The update-admission lease survives caller cancellation until blocking work
finishes, and responses retain their ordinary shape. Calls on one socket remain
ordered; independent sockets read while mutation turns or the app mutex are
occupied.

MCP `list_tasks` returns at most 20 full task rows, defaulting to 20. Its
optional `limit` accepts 1–20; `cursor` continues the same state/status/label
filter. The reply is `{project_id, tasks, user_session, next_cursor?}`.
Only an absent `next_cursor` ends the walk: a filtered page may be empty.
The existing store/project/filter-scoped cursor seeks below an immutable task
number, in descending order, so editing a task does not move it between pages.
Selection and the cursor probe precede timeline, identity and summary hydration.
`get_task` retains the complete task and timeline. This MCP paging contract
does not change the SPA's separate `tasks.list` RPC or its 100-row page requests.
Router `list_work` selects its compact fields directly from the accepted shared
board fragments, without materializing the full board first.

The tools an agent sees depend on its surface (`McpSurface`: `Coding`, `Router`,
`Project`), which a project's agents take from their persisted purpose (see
A project's agents below). Each action is a `BridgeAction` variant. The tool lists are
`coding_tools()`, `router_tools()`, `project_tools()`, `workspace_tools()` and
`task_tools()`. They cover conversation tools (`post_thread_message`,
`message_agent`, `set_topic`, `compact_self`, …; compaction in
`bridge/src/mcp/compaction.rs`), workspace tools (`create_workspace`,
`add_workspace_agent`, `list_project_agents`, `add_project_agent`, …) and the
tracker (`get_task`, `comment_task`, `move_task`, `label_task`, …).

`post_thread_message` and `message_agent` accept optional `attachments` entries
with `path` and optional `name` (#696). They share the task tools' file intake
in `app/tracker/agent_files.rs`: up to ten files of 50 MiB each, supported image
and video signatures or UTF-8 text, and regular-file reads without following
the source leaf symlink. Absolute agent-created paths are allowed by that
existing policy; relative paths must identify an existing stored attachment.
The bridge validates every source before copying bytes into its durable store.
Conversation records and delivery payloads retain the existing
`{path, name, mime, size}` descriptor, and receiving agents get readable copies
through the conversation's normal attachment homes. User-supplied attachment
paths retain their existing storage containment checks.
`thread.attachment` bounds both default and explicit reads to 5 MiB while
returning the full file size; the existing ranged SPA reader reconstructs
larger agent recordings without exceeding the transport message limit.

The task board's column and assignee carry routine status. Agent prompts and
MCP descriptions tell agents to comment on tasks only for a decision or
finding that changes the plan, a question, a handoff, or the final result with
its evidence. Questions about assigned tasks belong on the task, with
`mention_user` only when the answer is the user's call. Agents do not comment
just to say they are still working. The shared task-tools note and conversation
protocol carry this rule; wording tests keep them consistent with the tool
descriptions and assignment prompts.

**A project's agents.** A project's conversation owner (the run
`project.ensure_conversation` mints over the project's scratch directory)
holds exactly one pinned coordinator and any number of workers
(`bridge/src/project_roster.rs`, `bridge/src/app/projects/coordinator.rs`).
The owner persists `pinned_agent_id` and each agent its `project_role`; every
one of them still carries the `project-` id prefix, so nothing reads purpose
off the id. The owner is born with its pin; boot and every ensure settle an
older roster (its coordinator, else its primary, becomes the pin, and every
other agent a worker with its id, history and settings) and mint a pin only
where none can be kept, never by roster position. The pin is first in rail
order, so every verb that names no agent reaches it. `agent.remove` and
`run.abandon` refuse the pin and its owner with `conflict`; `agent.add` makes
only workers there. Recovery never retires the owner either. A boot or the
vanished-run sweep that finds the scratch directory gone creates it again
instead of abandoning or archiving the owner. A boot that cannot reach the
project's folder keeps the owner live, recognised by the project path on its
record, so the pin keeps its purpose and refusals. The same owner answers
once the folder is back. Only `project.delete` retires the pin. The coordinator is on the `Project` surface with the
`project_agent` prompt; a worker is on the `Coding` surface with the
`project_worker` prompt (`bridge/templates/project_worker.md`): it starts in
the project's folder, may write every source, and is refused a start while
that folder is missing. The daemon authorizes every call by the persisted
purpose (`AppState::agent_mcp_surface`). A worker's launch also declares the
coding surface to its MCP server in `BRIDGE_MCP_SURFACE` (codex through its
server `env` and `enabled_tools`), which `McpSurface::launched` only ever
narrows, so a copied value grants nothing.

**Project workers at work** (#533). Both working surfaces carry
`list_project_agents` (the owner's whole roster, the pin first, each with its
`project_role`) and `add_project_agent` (one more worker through `agent.add`,
unwatched unless asked, stamped `created_by`), scoped to the caller's own
project by its binding, so an agent in a named workspace can staff the project
too (`bridge/src/app/projects/agent_tools.rs`, `agent_writes.rs`). Source
administration, reclaim and `compact_agent` stay the pin's. `assign_task` and
`tasks.assign` take `{kind: "new_project_agent", agent_name, …}`: a new worker
on the task's project's owner, handed the task, linking no workspace
(`bridge/src/app/tracker/dispatch.rs`). `tasks.assign` takes the client's
`operation_id`; the delivery is queued under it, and asking again with the
same id validates the saved assignment target, actor, sender and note and
answers its durable delivery state instead of delivering again or making a
second worker (`bridge/src/app/tracker/delivery_acceptance.rs`).
Removing a worker retires its session, token and queued turns alone, and
resetting one replaces its own history and session: siblings and the pin keep
theirs though they share its folder, and the task it held keeps its record. A live worker — a session running, being
started, or owed a queued turn — holds every source of its project:
`project.remove_source` and a folder move through `project.update_source`
refuse with `conflict` naming the workers to stop, and while a move is in
flight no worker starts (`bridge/src/app/projects/source_holds.rs`). Workspace
reclaim never sees the project's owner, and reclaiming or deleting a named
workspace never retires a project worker, even one its agents made.
`project.delete` retires the pin and every worker, revokes their tokens and
queued turns, and removes the project's scratch; its sources stay.

**Waiting-on tools.** `create_task` accepts `waiting_on`; `link_task` adds
those edges atomically with its ordinary links. `unlink_task` removes only
waiting-on edges, and `list_task_dependents` pages same-project reverse digests
with an opaque scoped cursor and a maximum of 100 rows. Every endpoint is
validated before a write lands. `assign_task` accepts `retry_deferred_id` alone
to retry the exact current failed intent, retaining its target, note and receipt
identity. Assignment while blocked saves authority without enrolling or
waking its holder. Comments still arrive; Complete reminders and automatic
moves exclude blocked or undelivered assignments. A blocked user assignment
is not an assignment-only ask; explicit questions and existing watch/read/
dismissal marks remain effective, and unblock makes the assignment actionable.

**Tracking.** A task's `trackers` are the agents told about each change to it
(`bridge/src/app/tracker/notices.rs`; never the agent that made the change).
`create_task` tracks by default, assignment tracks the assignee, and every
other write tracks with `track: true` (`also_track` in
`bridge/src/app/tracker/tools.rs`). Tracking ends with the work (#444): the
store write that takes a task from live to Done or closed
(`Task::end_tracking_on_finish`, called from `save_activity_in_tx` and
`complete_review_in_tx`) removes every tracker and records one `untracked`
event each, by Build, with `by: "finished"`. The notice for that move or close still
reaches them, read from those events, and it names the move or close even when
the same write changed other things. Writes never put back a tracker list they
read earlier: `save_activity_in_tx` takes the stored trackers and applies only
the write's own `tracked`/`untracked` events, so a stale write after an off-lock
finish restores nobody. The inbox skips these `untracked` events when it picks a
row's last event and checks dismissal (`bridge/src/app/tracker/inbox.rs`).
Reopening restores nobody, and `track`
on the write that finished the task is ignored. An agent that tracks a finished
task explicitly afterwards keeps its tracker. `untrack_task` is for dropping a
live task early. At boot `Store::end_tracking_on_finished_tasks` clears, once,
the trackers left on tasks that finished before this rule. The user's watch
(`watched`, the inbox) is a separate field and does not change on Done.

### Relay and direct connection

Build maintains the crypto bindings in [transport/](transport/README.md) and
`bridge/src/transport.rs`. The historical [E2E-MVP](transport/E2E-MVP.md)
describes the envelope protocol; its older relay/presence topology does not
replace the signaling-only relay and API-owned device keys described here.
The bindings do not enforce replay protection or authenticate the outer
`route_to` field. Moving them into Build changes ownership and build inputs,
not the protocol, sessions or network behavior.

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
  queries. Each completed lookup logs one bounded, redacted round summary at
  ordinary diagnostic level: up to eight local interface ordinals with kernel
  ifindices, membership/send/arrival counts, retries, response and rejection
  categories, shared-receiver errors/lag, and applied timeout/elapsed time.
  Interface names, LAN addresses and queried names are omitted. A deadline with
  no observed matching response does not establish whether the browser answered
  or where a response was lost.
  `rtc/remote.rs` defers names from offers and trickle without holding
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
  Repeated global ARP configurations in multipart dumps are validated independently
  and folded using the maximum entry count and minimum thresholds.
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
- **TURN/ICE diagnostics**: the bridge correlates its process-local opening ordinal,
  remote ICE generation and TURN allocation ID. Opt-in vendored TURN diagnostics
  use a private hash key shared within the opening, so an endpoint can be compared
  across fresh allocations without logging its address. Each allocation assigns
  lifetime first-use ordinals for up to 256 peers, aggregating additional peers
  under an overflow ordinal. It records transaction outcomes and directional
  indication/ChannelData counters. Packets update counters silently after first
  use; periodic and terminal summaries carry those counts. Data rows do not
  consume the control-plane log budget. The dedicated diagnostic log target
  bypasses the general gatherer warning throttle and folds control-plane storms.
  ICE selection and failure snapshots record pair types and connectivity-check
  counters with opaque pair IDs. Statistics requests run outside the negotiation
  mutex; diagnostic rows are checked against the current peer and offer generation
  before logging. Connected events always record Carrying in the transport ledger,
  including while a restart offer awaits delivery. These are observations, not proof of why a
  provider rejected or dropped traffic, and do not change the retry state machine.
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
- **Superseded failed sessions**: `SessionPeers` closes a session's peer
  whose ICE connection state is `failed` once a newer open session presenting
  the same hint holds a peer (#373). It checks on every offer and whenever a
  peer's ICE fails, so a reload closes the old peer at once in either order
  instead of after the 20-second write stall. State callbacks only prompt a
  look; they never decide. The peer decides under its negotiation lock, the
  one every offer holds, in two reads. First it checks that the driver has
  delivered every have-remote-offer signaling event the connection's core has
  queued; `answer()` counts them under the same lock, one each time the core
  leaves stable. Then it takes one statistics snapshot and closes only if the
  ICE state the core recorded as it drained is `failed`. That failure belongs
  to the current ICE generation. Every native ICE restart happens inside
  `set_remote_description`, after the call has queued its have-remote-offer
  event, so a delivered offer means the core has drained since its latest
  restart. The agent leaves `failed` only through a restart, which queues
  `checking` behind the failure, and the core records ICE states in the order
  it drains them. A drain after the restart therefore leaves the record at
  `checking` or later, so a `failed` record cannot predate it. The lock keeps
  any restart out between the two reads. A failure recorded after the latest
  restart always prompts a look once its events are delivered: the `failed`
  event prompts it, and so does the restart's offer if it is delivered after
  the failure. So an idle peer that really fails is closed, including after a
  restart that leaves the agent checking and queues no `checking` event. An
  offer the core refuses after leaving stable is counted too, because it may
  already have restarted ICE (an empty password restarts it and is then
  refused). The vendored wrapper sends its driver no wake for a refused
  description and a failed core has no timer, so `answer()` wakes the driver
  itself by reapplying the configuration; the refused offer's event is then
  delivered and prompts the look like any other.
  An offer never closes the peer it is answering through; an offer whose peer
  was closed mid-answer answers through a fresh registered peer. Only the peer
  goes: the session stays open, and its next offer negotiates a fresh peer.
  Sessions without a hint are unaffected. The hint correlates sessions as a
  bearer value; it is not authenticated client isolation. Any session that
  presents a hint, including another paired client that has learned it, can
  get a failed peer of an older session with that hint closed. It cannot close
  a peer whose native core has not recorded an ICE failure since its latest
  restart.
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

`buildctl` (`tools/buildctl/`, launched with `scripts/buildctl --env <name> …`)
is the supported management entry point for authorized agents working against
non-production environments. Its checked-in registry declares channels,
services, isolated paths, public origins, hosted deployment branches and mutation
policy. Production is read-only. The Python/uv CLI provides `bridge` deployment,
status/logs, pairing, verified rollback and pruning; `app deploy` advances the
registered hosted branch, while `app status` reads the public version and latest
deployment workflow. An explicit `--force` uses an exact force-with-lease.

Task branches start from `origin/dev` and enter `dev` through the serial merge
gate. The dev branch runs its own deploy-only workflow on pushes: resolve the
triggering SHA, build the app/relay images, then roll out only successful images
to `build-dev`. Trusted dev pushers control that workflow and dev-only
credentials; no main-owned signal workflow or CI checks precede dev builds.
Rollout groups never cancel, and each rechecks the dev tip before applying
immutable image digests. The `development` environment must allow the `dev` ref.
After validation on the dev stack, `dev` is promoted to `main` by fast-forward.
Main's `CI and deploy` retains the full test graph; both production rollouts
require its validation aggregate, which accepts path-filtered skips but rejects
failed or cancelled suites. `buildctl app deploy` still fast-forwards `dev`.

Bridge deployment builds a resolved commit (default `origin/dev`) in a temporary
worktree under `nice -n 10`, stages immutable binaries in
`~/.local/state/build-dev/builds/<sha>/`, strips inherited `BRIDGE_*` settings,
and invokes the selected channel and public origins from the registry. It
installs/reloads/stops/starts only that channel's unit, then checks active state,
the running executable's SHA-256 and fresh relay authentication before recording
current/previous commits. First installation needs deploy → pair → deploy after
approval. An unrecorded legacy executable is reported from `/proc` as unmanaged;
its first verified replacement creates no previous record and never deletes the
external binary. [Working against dev](AGENTS.md#working-against-dev) documents
the policy: validate on `dev.getbuild.ing` / `relay-dev.getbuild.ing` before
production; moving `dev` auto-deploys the hosted stack. Production rules remain.

`BRIDGE_CHANNEL=dev` disables the updater independently of whether a binary
has release provenance: no checks, installs (even confirmed replacement),
schedules, staging, helper dispatch, recovery, probation or heartbeat. Its
`UpdateService` is an inert status service that reads no updater files and
calls no backend. This is separate from a production-channel development
binary, whose explicit release-replacement behavior below is unchanged.

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
  systemd user unit (`systemd.rs`) or a LaunchAgent (`launchd.rs`). Dev selects
  `build-bridge-dev.service` / `ing.getbuild.bridge.dev`; production keeps
  `build-bridge.service` / `ing.getbuild.bridge`. Both use their resolved log root.
  Set `BRIDGE_CHANNEL=dev` in the invoking shell for both `install-service`
  and `uninstall-service` to select the dev service; an absent channel selects
  production. Service installation persists the selected channel in its environment.
- `scripts/install.sh` is the public installer. It verifies and installs the
  binary, then runs `build-bridge pair` and `build-bridge install-service`.

### Bridge tests

The busy lock fixture includes MCP permit admission, blocking-pool scheduling
and all app guards in each frame, and separately times callers through replies
or completed status observation. Its cohort includes 168 MCP calls, 40 encrypted
RPC calls and 24 actual private `StatusPump` observations. All callers and
queued deliveries must finish before assertions. The five-second maximum
applies to all frame and caller totals, alongside the hold percentile and
maximum gates. Status timing covers observation work, not watch-notification
delivery latency.

Three focused MCP timing tests reuse that fixture's 200 conversations, 190 Git
worktrees, 700 initial tasks and 12 simulated running agents. They measure
`list_task_dependents`, `search_conversation`, `read_review`, `read_comment`,
`list_work` and `list_workspaces` with only the read cohort, with the unchanged
writer/status/RPC cohort, and with that same busy cohort plus concurrent
`list_tasks`. Each mode captures 24 decoded socket replies per read type,
rotates the first action across readers, and enforces the 100 ms caller p99
and maximum for every cohort MCP read and ordinary read RPC.
Per-type overlap counters require every read type to start while writers are
outstanding, and while task-list requests are in flight in the final mode.
`.github/workflows/timing.yml` builds the release test binary before running
these three tests serially alongside the existing busy lock-hold regression.
Ordinary tests verify that the recipes use the independent lane and return
their saved task, comment, conversation, workspace and review witnesses.

The test-only `timing/phase_samples.rs` collector reports exact p50, p95, p99
and maximum durations. MCP read samples separate blocking-pool scheduling
(`frame.pool_wait`), per-type admission (`read.class_wait`) and SQL connection
acquisition (`read.sql_wait`). Synchronous `read.admission`, `read.query`,
`read.run` and `read.finish` spans are exclusive of nested phases; their
`.total` counterparts include nested work and record one sample per stage.
`reply.serialization` measures the socket reply encoding, while `client.total`
includes socket submission through the parsed reply. These diagnostic fields
compile only in tests.

The focused `busy_mcp_lists_finish_within_100ms` release regression reuses the
main busy fixture with 700 tasks, 200 conversations and 190 worktrees. Twelve
readers request both MCP lists alongside the normal writers and status
observations. It measures through socket transfer and reply parsing, enforcing
the 100 ms maximum and p99 for each page. Test-only phase tracing separates SQL
methods (including record decoding), row construction, snapshot projection and
reply serialization. The release timing workflow runs this regression alongside
the existing busy lock test. Normal tests cover bounded page work and keyset
continuation independently of machine timing.

Unit tests sit beside the code (`#[cfg(test)]` modules, and directories such as
`bridge/src/app/tests/` and `bridge/src/store/tests/`). Integration tests are in
`bridge/tests/`: the wire contract (`api_contract.rs`), relay (`relay_*.rs`),
WebRTC (`rtc_peer.rs`), presence (`presence.rs`, `device_presence.rs`), E2EE
interop (`interop_python.rs`, using the required in-tree `transport/python`
reference through frozen uv resolution), compatibility vectors (`transport_vectors.rs`),
store migration and the complexity ratchet (`complexity_ratchet.rs`). The
complexity threshold is in `bridge/clippy.toml`. Missing uv or reference files
fail the actual Rust/Python round trip; the test does not silently skip. JS/Python
interop and Python/JS vectors run in the dedicated transport CI job. These
are JS/Python and Rust/Python checks, not a direct JS/Rust runtime exchange.

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
dependency is self-hosted. `@build/secure-transport` is a `file:../transport/js`
dependency within this checkout (`spa/package.json`). SPA and web commit their
portable npm locks; install `transport/js` from its own lock before either
consumer, using Node 22.23.4/npm 12.0.2 and the install sequence in
[AGENTS.md](AGENTS.md#working-rules). App and QA images build the binding from
this same checkout; the Build commit pins the shipped transport. The Vite libsodium shim uses SPA-local libsodium 0.7.16, matching the
transport lock; Node resolves the binding's dependencies from `transport/js`.

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

### Project workspace surfaces

The project view (`spa/src/views/projectView.js`) has Tasks, Changes, Files and
Workspaces faces on one kept shell. Tasks remains project-wide and is the
canonical project URL's default. Workspaces lists named child workspaces only;
its lifecycle, size, filtering and Reclaim controls do not describe the original
sources. The project has no synthetic workspace record. `projectRailState.js`
remembers local returns by device/project, including the Changes source and
commit; explicit links take precedence. The toolbar links the project
ancestor, offers a local workspace selector and opens a global switcher with
the leading left-chevron action. `toolbarBreadcrumbs.js` derives ancestry
from the URL; `navigationModel.js` and `navigationRoutes.js` share cached
destinations and return behavior. The original-source destination says
“Project workspace”; project bars and inbox heads retain the project name.
Named children use their given workspace names; their collection is
“Workspaces” and creation is “New workspace”.

`spa/src/views/projectChanges.js` mounts the shared kept directory panes from
`workspaceChanges.js`, one per configured source. Each Git pane works on that
source's actual checkout HEAD, with ordinary history and commit-all actions,
without isolated-workspace review semantics. The context names the shared
checkout, configured base and cached sync outcome separately from HEAD. Plain
folders remain visible with an honest no-Git explanation and no initialization
action. A Changes diff opens Files with the same source, path and line.

Project list cache writes reconcile added, relabelled, removed and moved sources
in place. Surviving panes keep their drafts, selected commits and scroll; moving
a source gives it a new location namespace. Face changes retain the shell's
conversation and console. The project-worker rail is described below.

The project and its project-hosted task pages share `console: {kind: "project"}`
in `shell.js`. The bottom Console tray opens the existing console and never
creates a shell; the project rail has no separate Terminal entry.
`projectConsole.js` paints source-labelled tabs from each
source's `terminals` cache record at device/project/source/captured-path addresses.
Source terminal roster subscriptions use realtime delivery at foreground priority.
`projectConsole.js` re-reads only its project/support/source-terminal cache records;
create and close commit and read back independently of background collectors,
guarded by an applied generation.
A remembered source picker chooses only the next new shell. It stays visible
even with one cached source, with the Files folder glyph inside the control
before its name. Its fixed width avoids layout shifts as sources change.
Changing it or changing faces leaves the selected shell and pane standing.
Explicit tab close uses the tab's captured source assertion and updates only that roster. Collapse
and navigation detach client screens without closing server shells.

`projectTerminalSupport.js` remembers `term.projectSources` for cold rendering.
`projectTerminalRpc.js` checks the current compatible greeting and exact cached
source membership before list/create/attach/close, including reconnect attach.
Unsupported bridges retain cached tabs and an update notice, receiving no new
terminal selectors. The shared terminal manager fences requests and pane input
against device changes. Pane cancellation crosses connection waits, source/greeting
validation and attach replies; detach releases only the requesting registration.
Failed or cancelled pane setup releases every acquired terminal resource. Channel
recovery stays in the terminal lifecycle; the project console neither reads
connection status nor changes its presentation on disconnect/reconnect.
`cacheSync.js` prepares original-source subscriptions with the ownership lists
when `changes.projectSources` is present. On sync/reconnect, `syncPhases.js`
reads all source terminal rosters after the foreground and inbox, through the
same two background streams as checkout warming. Their exact terminal record
identities share the planner's completion and visibility promotion, so an
unrelated source reply never blocks a newly selected foreground. Source push
and list writes check membership/location transactionally and keep equal IDs on
other devices and paths separate. No project terminal polling is introduced.

Commit drafts keep device and account-epoch ownership in the in-memory stash
as well as their device-scoped `build-ui` records. Old account panes cannot
publish or clear a replacement account's draft after reset. Account reset
retires live UI writers and clears the page-exit journal before wiping `build-ui`,
so a remount cannot replay the previous account's pending text. A moved or removed
source's saved commit message remains under its old project/source/location
address. `projectChangesDrafts.js` exposes those messages in read-only recovery
fields labelled with the old path, for explicit copying; it never attaches them
to a replacement source's commit action. Recovery listens to UI-store writes so
a draft flushed during reconciliation becomes visible, and a restored exact
source/location resumes normal draft ownership. Source/location namespaces are
independent of bridge capabilities. Older project-only drafts appear as read-only
recovery fields with an unknown source location, for explicit copying rather than
assignment to today's primary source. Changes deep links keep their requested
source and commit until the source location resolves from the project cache;
commit-only links wait for the default source.
If a requested source is absent from a populated cached roster, surviving
sources still mount alongside a visible missing-source warning. The URL and
requested commit remain pending until that exact source resolves or the user
chooses a surviving source; the missing source's commit is never applied to it.

Source Git records use `directoryCacheId`'s project/source/captured-location
entity inside the device cache scope; status keys alone never select a source.
`cacheSync.js` verifies the source against cached project membership
before writing pushed records and keeps legacy project records separate.
Source-qualified subscriptions match device, project, source and captured path.
Git panes select event coverage separately from their stable storage/RPC scope:
bridges without `changes.projectSources` retain board/state subscriptions, while
source-capable sessions use exact source/Git subscriptions. Greetings reconcile
that coverage without moving draft addresses or weakening captured-path guards.
The mounted pane writes read results to the cache before painting; disposal and
conditional writes fence obsolete reads. Source moves cannot apply an old reply
to the replacement namespace. `projectChangesRpc.js` captures the opened source
and rechecks membership/location and current capabilities at dispatch, including
after a queued greeting. Stale source writes refuse instead of following a move.

### Project agent rail and rosters

The project and its project-hosted tasks show one protected pin above the
separator and every project worker below it, including unwatched workers.
The pin's identity comes from `pinned_agent_id`, independently of roster
ordering. Named workspace pages show their own agents and the project pin;
project workers stay on project hosts. The assignee picker includes workers
from that exact project's complete cached roster, with new-worker assignment
separately gated by `tasks.assignProjectAgent`.

`projectAgentsCache.js` stores complete rosters at
`deviceId|entityId|project-agents` separately from legacy pinned-only rows.
The per-device `project-agents-support` record holds the greeting's
`agents.projectWorkers` and `tasks.assignProjectAgent` support. Rendering reads
support and roster together; a valid roster must name that exact owner and a
pin present in its agents. Without cached support it paints only the legacy
pin. Current greetings additionally gate worker creation and new-worker task
assignment, including a retained picker caller after a capability downgrade.
The pin's custom tooltip retains its “Project agent” purpose alongside the
cached agent identity. Conversation bubbles show the agent's name, current
work, turn runtime or idle state, harness and model from cached roster data.
The tooltip opens on hover or visible keyboard focus, closes on click or
pointer leave, stays inside the viewport and does not add a touch-only
interaction. It replaces the bubble's native title. An unnamed pinned agent
uses the same “Project agent” label in its tooltip and conversation header.
Hover and keyboard focus retain separate owners across kept-button repaints.
Long work text is ellipsized so the fixed facts fit without scrolling, and
models without a compact badge retain their cached label or id.
The conversation header keeps the agent name above the smaller current-work
line; only the work line erases and types when its topic changes. A missing
topic retains the existing shimmering “Starting” text, regardless of lifecycle
state. Clearing a topic erases back into Starting. The work line stays mounted
and keeps its space throughout, so the name and adjacent controls do not shift.
An explicit cached
`projectWorkers: false` shows “Update this device to use project agents.”
on a project host; unknown support and workspace pins keep
that notice absent. Rendering the notice does not replace the mounted
composer or thread, and never waits for connection state.

`cachedFeedView()` projects complete rosters onto `projects[].projectRoster`;
`projectRosterRows()` supplies complete owner rows to lineage, task activity,
attention and sync without replacing pinned-only inbox summaries. Overview
reads the same complete roster directly. Overview, lineage and the task feed
hear both roster and support invalidations; task activity follows canonical
conversation addresses, and watched-task running state follows the exact
worker or persisted pin. Route discovery keeps its separate legacy owner-list
path, so reading every worker roster never delays finding the selected
conversation. The rail's `railWorkItem.js` reads the same complete
record on project hosts and filters it to the exact pin on a workspace host.

The inbox derives project activity from the same complete roster and cached
lineage as chat, including workers absent from pinned-only board rows. Lineage
deduplicates canonical conversations within their owner before building child
lists or computing descendant totals, and resolves creator aliases to the same
canonical member. Both chat's Agents entries and inbox counts therefore list a
spawned conversation once, even when its roster repeats it. Derived
rollups decorate feed copies of both board agents and complete project agents;
they never rewrite cache records. A complete project's running count sums each
independent roster agent's own running loop and running panel rows, counting
roster descendants on their ancestor's panel only once. An idle pin with three
running workers says “3 running”; its own running loop adds one. Independent
project workers also contribute, while workspace and legacy roster counts keep
their existing per-member meaning. Rows with running activity or conversation
history omit the empty “Getting started” fallback, retaining any Git or
pending-operation facts. History comes from the conversation's own session,
with legacy activity timestamps used only when that session is not explicitly
empty; project-wide activity and creation anchors do not establish history.
Topic and ordering still come from the pinned identity and the project
conversation's own session.

Roster pulls use the sync planner and its two shared background wire slots.
The selected project's roster has foreground authority; unrelated held roster
replies cannot delay selected thread, Git or task recovery. Landing a complete
roster extends that pass's conversation jobs before it reads worker histories.
State and thread invalidations refresh the complete record through the same
scheduler. Roster admission captures every named thread generation and checks
the roster write and those generation records in one transaction; a late
pre-reset reply cannot restore a retired worker conversation. Removal and
reset target the exact worker, preserving sibling and pin drafts.

When a project/workspace host changes while the route explicitly selects
the same canonical pin, the shell retargets the mounted rail instead of
disposing it. The composer, thread and focus remain mounted throughout
navigation; the viewing
workspace and host-specific Overview/console change with the host. A different
agent, device or repository still selects its own controller.

`projectActor.js` supplies shared purpose checks for task rows, comments,
avatars and thread notices. Durable `TaskAgentIdentity.project_role` survives
removal; older stored identities are backfilled from a persisted roster role
when one is available. A recovered project purpose clears synthetic workspace
identity fields, including fields retained by older snapshots: the owner's
scratch directory is not a named workspace. A named worker keeps its own name
and harness mark. An unnamed worker uses its captured ordinal and harness;
an older captured
identity with unknown purpose uses the same generic presentation unless exact
cached membership confirms the pin. Deleted identities retain their label and
have no conversation link.

### Files

Workspace directory edits publish their completed workspace result to the
shared `workspaces` cache list as well as the settings sheet's detail record.
The mounted workspace view listens to that list: Files reconciles its roots
in place, and Changes reconciles its tabs and kept directory surfaces. A
pending directory is listed immediately and loads its tree or Git surface
when the cache says it is ready. Removing a directory disposes its readers;
other roots, the open file, the selected surviving tab and the console retain
their state.

The Files explorer shares directory upload and New folder actions across
workspace sources, project sources and checkouts. `spa/src/core/filesUploads.js`
mounts them from cached upload capabilities; `fileUploadRpc.js` checks the
current greeting when starting a write. Their SVG actions appear on hover or
keyboard focus; on coarse pointers `longPress.js` reveals one directory's
actions after a stationary hold, preserving ordinary tap navigation.
`fileUploads.js` owns an in-memory
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

Animated agent canvases share a frame loop and retain their measured CSS box
between frames (`agentCanvas.js`, `canvasGeometry.js`). A ResizeObserver
invalidates the measurement and repaints even an idle canvas when its size
changes. Animation therefore does not force layout after every cache paint.
Engines without that observer retain the per-paint measurement fallback.

Thread attachment tiles use cached message descriptors and held media bodies
for user messages, agent replies and expanded outgoing agent messages (#696).
The shared attachment loader fills missing bodies; the same image lightbox and
download controls serve all three. Message role and connection status do not
gate attachment display.

**How it is built:**

- **The cache** is `spa/src/core/localCache.js`: one IndexedDB database
  (`build-cache`) whose record keys are addresses of the form
  `deviceId|entityId|kind|sub`. It has `readCached`, `writeCached`,
  `writeCachedIfNewer`, `mergeCached`, `evictEntity` and friends.
  `subscribeCache(prefix, listener)` announces every write to listeners on a
  matching prefix, and a `BroadcastChannel` carries the announcements to other
  tabs.
- **Pending writes** (#740, wire 3.24.0). `writeOperations.js` persists each
  normalized intent before transport in separate versioned operation/lease
  stores inside `build-cache`. The authenticated app shell supplies only the
  account UUID; operations are keyed by account, paired device, user principal
  and operation UUID. Without that identity the pending service is unavailable.
  Boot activates the account's projections before mounting cache readers.
  `readCached`/`readCachedMany` compose ordered local layers over canonical
  records; mutation helpers read the canonical records separately. Replica
  eviction never deletes pending intent. Explicit account reset clears it.
  BroadcastChannel announces committed operation changes, and an IndexedDB
  per-device tab lease suppresses duplicate transport. A single worker polls
  status in batches of at most 128 with jittered 1/2/5/10/30-second backoff.
  Each subscription replaces the whole set with the oldest 128 outstanding
  submissions; overflow stays on status polling, and terminal outcomes refill
  the set. Subscribe pagination continues through status with the same IDs.
  Subscriptions belong to the opening and carrier, so even a same-epoch carrier
  change re-establishes status and subscription coverage.
  Durable ack resolves the action successfully; a receipt does not. Loss,
  timeout and restart retain the same operation identity and accepted layers.
  A confirmation invalidation retains its layer until a fresh, guarded canonical
  read covers the publication's owner kind, ID, generation and sequence.
  Failed/unapplied removes only its layer; partial/unknown retains evidence.
  Named refresh obligations survive reload independently of overlays. This
  foundation consumes inline results; an unknown result reference remains
  unresolved and cannot become success or authorize retransmission.
  Expired-result tombstones retain their outcome and effect. Proven
  failed/unapplied tombstones remove only their layer and restore the draft;
  the expired operation ID never resolves successfully or executes again.
  Task comments and thread posts display cached operation errors beside the
  composer, retain the draft, and distinguish a new-action Retry from same-ID
  Check again. The method allowlist is empty until bridge execution slices enable
  methods; typed domain effects are extended together with those slices.
- **Compaction observations** (`compactionCache.js`, wire 3.22.0) refresh from
  authoritative scope reads and `agent.list` after `compaction.changed` or
  subscription acceptance. The invalidation names no replicated values.
  Because held ownership lists can be partial, the service refreshes all held
  compaction dependencies on that device and the invalidated settings scope
  even when cold. A burst shares one pass and one trailing pass; a new session
  bypasses an old stalled pass. Ordinary push fences and atomic record-write
  comparisons protect newer cache data, and greeting authority is rechecked
  inside the transaction. Agent updates merge only compaction observations,
  retaining unrelated roster fields and clearing removed overrides. Rendering
  support is the per-device `compaction-support` record from
  `compactionSupport.js`; dispatch requires the current greeting separately.
- **Compaction editors** (#661). `scopedCompactionEditor.js` mounts the same five
  harness rows in device, project and workspace settings. It reads the scope's
  settings record and `compaction-support`; existing readers and invalidations
  own network refreshes. Drafts stay in the mounted editor, and project settings
  retains its section across surrounding sheet paints, with stable action IDs for
  the sheet’s keyboard focus restoration. Positive safe integers
  set an override; Inherit sends null. Setters use the current adapter, and replies
  publish only while the captured record and push fence still hold. No compaction
  preferences are stored in localStorage. The conversation menu uses the digest's
  inherited observation and preserves its existing scalar override. Legacy numeric
  observations remain read-only and can be cleared separately; absent observations
  show the older-bridge update copy without start/resume timing claims. The gauge
  measures usage against applied policy (translated trigger for native enforcement),
  labels applied-at-start evidence and pending settings separately, and falls back
  to the model window when active policy or native trigger is unknown.
- **Waiting-on capability history** (#564). `taskWaitingOnSupport.js` keeps
  the per-device `task-waiting-on-support` cache record beside other capability
  flags. `waitingOn` describes the latest greeting for rendering Waiting on /
  Blocks controls; `everWaitingOn` records that this device has advertised
  `tasks.waitingOn` at least once. Every authenticated capable greeting for the
  original account/device sets history, even if a newer greeting has already
  superseded its hello or overtakes its storage transaction. Incapable
  greetings never clear it. Evidence updates in memory before persistence;
  a refused write cannot erase it within that page's lifetime. Every
  authenticated capable greeting retries persistence. The existing `build-cache`
  BroadcastChannel shares monotonic evidence with open tabs that are listening,
  so those informed tabs also retain it in memory if persistence fails. Admission
  distinguishes a committed absent record from an unavailable/error read;
  unknown or malformed history refuses
  new-workspace assignment under an incapable greeting with the update message
  and a retry/refresh hint. Account reset retires pending evidence and its
  in-memory history. Existing cached `waitingOn: true` records count as
  history without a schema bump. Task lists, detail and reverse pages remain
  the sources of displayed dependency facts; assignment admission does not
  arbitrate task replicas or maintain per-task dependency knowledge.
  **Accepted residual risk:** history belongs to the paired device UUID.
  Re-pairing the same machine creates a fresh UUID, so its previous capability
  history does not follow it. Downgrading the bridge and then re-pairing it are
  two deliberate actions on an unsupported downgrade path; the old bridge
  cannot enforce waiting-on prerequisites whatever the client does. This
  limitation is accepted; the client does not link identities across pairings.
  **Accepted residual risk:** evidence that could never be written to storage
  lasts only for that page's lifetime.
  Open listening tabs can retain shared evidence for their own lifetimes, but
  new or uninformed tabs cannot recover it, nor can a reload after all informed
  page lifetimes end. The residual requires refused storage, a reload or
  uninformed tab, and a bridge downgrade. The client cannot retain across reload
  what the browser will not persist. Retrying on every authenticated capable
  greeting and sharing over the existing channel mitigate this limitation;
  there is no new channel or durable fallback.
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
  ordered pass per device. `subscribeConnectionSync(listener)` immediately reports
  a boolean and returns an unsubscribe function. Only the connection status
  ring consumes this signal. A connect/reconnect pass starts the active period;
  each device captures its pending/running push jobs and catch-up rounds after
  its final chained pass. The period ends when every connected device's own
  snapshot drains, including deferred work spawned by captured body jobs.
  Captured payloads and catch-up goals are fixed; later arrivals coalesce in a
  separate trailing job/round and cannot extend the snapshot. No extra sync is
  requested for the signal. Queues from retired sessions, restored paths or
  stopped lifetimes do not hold it active, even if an abandoned RPC never
  answers. Connection-pass requests release their own ownership token on every
  exit, including exits that start no pass. Queue and pass mutations notify
  directly, without polling or delaying sync. Retained
  historical repair debt (`repairThrough`) is excluded; later pushes and tab
  return passes do not restart an idle ring. Standing down the sync layer clears
  the signal. The three ownership lists are requested together
  and written independently as they land. `syncPhases.js` then reads the open
  conversation, the open checkout and project's first task page, the inbox's
  other conversations and remaining task pages/review metadata, and finally
  source terminal rosters, then background checkout records and bodies.
  `syncBackgroundStreams.js` drains that final phase through two streams per
  device, retaining each checkout's
  read/write order and letting a free stream advance past a slow checkout.
  Inbox and push-triggered background RPCs share those two wire slots per
  device/session/restored path. A failed first task page still waits for its
  parallel columns read before the inbox phase ends. Foreground reads use no
  background slot; queued reads carry the record identity that the foreground
  jobs name and recheck promotion when they wake. All reads needed to finish
  that record share its scope, including lean thread activity repairs, so a
  selected record never waits through its own foreground barrier. Push readers
  follow the active planner's priorities for their captured
  device/session/path/lifetime. Only pass readers await their owning pass's
  barrier; a serialized push can itself be the work that foreground joins.
  Stop or path/session replacement discards queued
  reads and abandons the old slots. Only the visible scope is promoted;
  mounted panes describe their shown trees, Changes and remembered conversation
  through `syncVisibleScopes.js`. Mounted source directories supply their own
  fenced cache readers where no board entity addresses those records. Route
  changes interrupt the pending order,
  including while an unrelated background reply is still out. Task pages
  resume the same cursor and removal spans across phases (`syncTaskPhases.js`),
  sharing a session-fenced queue with coalesced push reads. A refresh requested
  while the same session's pass is running queues
  one follow-up pass; all requests during that pass share the follow-up, and
  their promises settle with its result. The current pass finishes before the
  follow-up starts. Stopping sync or replacing the session discards queued
  work and prevents the old pass from sending remaining reads; replies already
  in flight stay fenced from writing. Coordinated diff reads replace inactive
  pending loaders so a restarted pass or foreground reader cannot share a
  stopped pass's cancelled answer. Tabs share the Web Lock `build.cacheSync`,
  and the holder syncs for all of them. It holds three change subscriptions
  per device: `s-inbox`
  (realtime), `s-background` (git, files and shells on a 30 s cooldown) and
  `s-active` (the routed workspace, realtime).
  Unpushed metadata reads send `if_diff_key` only when the held record has the
  complete diff shape; the lighter pushed summary requires a full shape read.
  Commit patches exceeding the warming cap leave only a `patch-over-cap`
  marker per entity/hash. Later passes and pushes skip those hashes; explicit
  review reads still fetch uncapped patches and pages. Markers follow the
  newest twenty unpushed hashes and the workspace's eviction and expiry rules;
  failed reads without size evidence remain retryable.
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
  unobserved in the `feed`, so the row's own record paints. Board admission
  reads its canonical row and thread guards inside the same feed write
  transaction that checks reset generations and stamps row observations. It
  does not capture each guard in a separate transaction or retry the whole
  board when a concurrent thread tip changes one record. Stopping sync
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
  A new session or restored path also validates held working diffs for the
  routed entity and every shown Changes pane on that device in the foreground.
  These conditional shape reads reuse the held diff key and leave accepted
  bodies intact when unchanged. Their cache writes retain the session, push and
  atomic record-write fences; ordinary file recovery debt does not re-arm them.
  Late background subscription coverage re-arms this validation to close the
  gap between a recovery read and the bridge holding its subscription.
  A conversation's forward read (`syncThreadWindow` in
  `spa/src/core/threadSync.js`) only
  carries items made past the cursor. When its page shows the conversation's
  counter moved on a value none of its items wears, an item under the cursor
  changed in place. The record then owes a repair (`repairThrough`, written
  with the cursor) until the newest `REPAIRED_THREAD_ITEMS` (50) held items
  have been read again at or past that counter.
  Only the forward page's snapshot counter advances delivery coverage; a
  later repair or exact-copy read cannot advance it over unseen creations.
  Empty in-place-only forward pages commit that coverage together with repair
  debt. Capped sparse repairs fetch omitted held representatives through exact
  one-item, unprojected pages (at most 50); failed or mismatched copies leave
  debt for the next bounded attempt or recovery pass.
  Sync and load-older pages request `activity: "newest"` only when the device
  advertises `threads.leanActivity`. Cold sync and conversation-open reads
  then request 23 messages, measured to fill the phone conversation even at
  the dense task-notice row height, plus one newest activity item per run.
  Legacy cold reads and warm catch-up retain 100; repairs retain 50.
  A detail filter hiding the entire held window still renders Earlier when
  the cached window says older items remain, so older dialogue stays reachable.
  A leading folded run on a newest page
  means `has_more` describes omitted activity, so held message context survives;
  load-older seeks above that run's digest start. A lean repair refreshes any
  omitted recent held activity through exact one-item `thread.activity` reads
  before clearing its debt. A closed run is complete only when distinct held
  activity covers its digest span and census; sparse warm catch-up fetches the
  missing interior even when the first row is held. Without an exact row census,
  historical runs are fetched conservatively. Closed runs open asynchronously
  with the existing throbber until their cached rows paint, and offer inline retry on failure;
  the live tail continues opening from the conversation window. Tail detection
  compares creation sequences: in-place updates advance delivery without
  closing a run. Fetched bodies merge newer window copies by update sequence
  before rendering, including folded descendants; their creation span can
  cross other runs, so window membership follows owned rows and fetched keys.
- **Pushes**: `watchChanges()` in `spa/src/core/changeEvents.js` registers
  subscriptions and routes each `changes` flush to the appliers (`APPLIERS` in
  `spa/src/core/cacheSync.js`). Most fields carry bodies that are written
  straight into the cache: `state`, `thread`, git `status`/`log`/`unpushed`/
  `diff`, the files root listing, and `terminals`.
  `pushQueue.js` bounds each device/session/path to one body applier and two
  deferred enrichment appliers. Compatible pending entity snapshots and thread
  spans coalesce. Numeric revision-only board hints share a high-water job and
  allow mounted priority across intervening hints. Board list replacements,
  removals, unknown board payloads, roster generation changes and resets retain
  their order. Unknown empty spans never become delivered coverage when folded.
  The body lane writes conversations before state and session summaries, and
  also writes terminal rosters without waiting for enrichment RPCs. The shown
  conversation's entity has priority, with an ordinary turn after four promoted
  jobs. Within an entity, its mounted conversation's carried tips land first. A carried
  thread page publishes its items and retention floor in one transaction. An
  empty or gapped tip publishes an advancing floor before its deferred repair,
  so a held RPC cannot keep deleted history visible. Roster/review reads run behind bodies with one pending refresh per key;
  a key never has two enrichment appliers running together.
  `threadCatchups.js` separately coordinates pushed catch-up/repair reads:
  one active round and one coalesced trailing high-water per conversation and
  generation. It rechecks canonical coverage before dispatch, skips already
  covered goals, and allows two attempts per goal and cache lifetime. Repeated
  unchanged tips do not renew that budget. An untagged post-reset hint probes
  held coverage without borrowing its ambiguous counter. New evidence, eviction, reset or a
  new session can. Ordinary recovery passes can retry retained repair debt.
  Catch-up work does not acquire a roster/review enrichment worker. A mounted
  conversation's page and activity reads bypass background wire admission even
  between passes; mounting it or changing routes wakes queued readers. Stop
  and generation replacement settle abandoned round waiters immediately.
  `threadCatchupsInFlight()` reports these active/trailing rounds separately.
  Finished-state eviction invalidates the entity data lifetime without removing
  its feed row. Deferred reads retain that fence through cache commits; a
  conversation reset also cancels its queued old-generation catch-up. Final
  review metadata belongs to the project and still refreshes after workspace
  eviction; the evicted entity's complete roster stays cold.
  User actions bypass these queues; neither their RPCs nor their cache writes
  acquire a push slot or wait for a batching timer. Awaiting a push still waits
  for its own deferred fields, without holding a body slot during those reads.
  `pushWorkInFlight()` counts pending applier jobs before IndexedDB admission.
  Feed and route consumers share an address index (`feedCacheRecords.js`) and
  stable projections (`feedCacheReader.js`, `cachedRows.js`). Concurrent readers
  share the initial row/owner read; auxiliary feed and support roots refresh
  independently, so route discovery never waits for those background reads.
  Mounted lineage takes rows and maker support from the same snapshot and
  responds to prefix invalidations, without a separate support read per hint.
  Later announcements reread only named records, in batches of at most 64.
  Unrelated writes reuse the held snapshot. An entity-prefix eviction removes
  only that owner's rows and rosters. Active reads and owner/auxiliary joins
  retain a departure fence so a captured response cannot restore the old owner;
  a fresh reader can observe its recreation. Device-wide and global-root resets
  rebuild the index and fence combined snapshots across the reset. Reads still
  include pending-write overlays and cross-tab changes; a failed read leaves
  the projection invalid for retry.
- **Chat read marks** (#473) report the viewport sequence and window floor
  only while the reader is present. A known cursor can establish read progress
  even if the cached unread count is zero. `chatReadReporter.js` confirms
  read/floor pairs only when the cached roster cursor covers them. Accepted
  reports that did not advance the cursor are suppressed at that cursor and
  reconsidered after cursor progress. It suppresses covered reports in flight
  and backs off refusals
  from one second to a thirty-second cap; later paint, scroll and return events
  drive retries. `entity.seen` emits the owner's updated `state` row after
  storing the cursor. For older bridges, `inboxSeen.js` follows a successful
  mark with `board.list` when that owner has a cached row, including owners in
  `runs` outside inbox `items`. It serializes refreshes per owner and merges only attention/unread fields
  into an unchanged row with the same roster generation, preserving Detail
  fields. Identical results cause no cache write; cache announcements redraw
  badges. Passive clients on
  other devices need the bridge's state push.
  Inbox uses the same mounted-thread visibility and focused-reader predicate
  as `agentRail.reportRead` (`syncVisibleScopes.js`). While that conversation
  is being read, a newly cached count is suppressed on every feed paint,
  including before `entity.seen` answers. Presence changes synchronously
  rederive the held feed; hidden, collapsed, TUI, overview and absent-reader
  states restore normal cached unread. This is a display-only symbol on
  derived agent digests, keyed by device, owner, canonical conversation and
  thread generation. Raw counts remain unchanged for history landing and read
  reports, and local presence is never written to the replicated cache.
  The bridge's read-window guard uses the same watch-scoped cursor and full
  conversation history as its unread count (#704). Pre-watch messages below
  the window do not block reading current news, while unread messages outside
  the resident tail after restart still block skipping over them. The published
  history index supplies that check; before publication, the store supplies
  omitted history. An unavailable history read preserves the cursor.
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

Chat openings use a visit state (`spa/src/core/chatLanding.js`) independently
of the retained transcript DOM. They land on New with 12 pixels of headroom,
or at the end when nothing is unread; scroll positions are never restored on
open. While an unread cursor is missing, the slice reaches the oldest cached
row and the opening stays at the start rather than skipping to the tail.
Missing transcript/cursor data and unread history pages can correct that
landing until the reader moves. `agentRail.js` loads older cached transcript
pages back to the read cursor. Its call sites wait for opening layout and
history before reporting read, bounded by `CHAT_LANDING_READ_WAIT_MS` (5 seconds);
settlement, reader takeover (even input without scrolling) or timeout releases
reports of the rows the viewport has reached. The read calculation
itself is unchanged. Roster hydration rebinds the transcript subscription when
the transient reader becomes the canonical history, even at the same address.

### Sync timing diagnostics (#873)

Settings → Diagnostics reads a tab-local, bounded `sync` record alongside the
existing connection events. `core/syncDiagnostics.js` retains eight ring periods,
32 detailed passes per period (with a total `passCount`), 64 stage spans per pass,
and the five slowest recorded RPCs. Unsettled detail is capped at 32 requests
per period; overflow retains aggregate counts only and ages are labeled as sampled.
The record uses monotonic offsets, fixed enums and
numeric counters only. Device summaries are unlabeled; no device/session/entity
IDs, parameters, paths, bodies or raw errors enter the timing record. Existing
connection events retain their previous schema.

`cacheSync.js` stamps ring, pass, stage and finite queue capture/drain boundaries.
The held-body count is reserved as null until a cheap synchronous source exists;
diagnostics adds no inventory scan or storage wait. Selected
conversation counters describe active, retained Recent and project-owner rosters
(the categories may overlap). `sessionRpc.js` stamps queued/admitted/receipt/
settlement offsets and exposes aggregate SQL queued, admitted and timed-out-held
credits independently of pending reply promises. A local SQL admission timeout
(`deadline: "admission"`) records the `admission-cancel` outcome. Other transport
failures have a null outcome. SQL snapshots cover registered RPC sessions; live
device counts come from the sync context registry.

Active request ages are computed only on report reads, using the mounted panel's
existing refresh. Copy and Share include the timing record; Clear resets its
history while preserving live counters and an active ring. Reload discards it.
No new timer, persistent store or connection-state UI is introduced.

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
  The private connection trace records answer receipt/application, candidate
  acceptance and local candidate type/family, alongside gathering, ICE and
  connection transitions. Native failures use fixed exception-name categories;
  addresses, SDP and native error messages never enter these added trace lines.
- **Session**: `openSession` (`spa/src/core/session.js`) combines
  `spa/src/core/sessionRpc.js` (encryption, pending calls, receipts, pushes)
  with `spa/src/core/sessionSwitch.js`, which sends `rtc.*` over the rendezvous
  and everything else over the peer.
- **States**: a device's lifetime is `spa/src/core/deviceLifecycle.js` (`new` →
  `available`; unavailable as `away` / `blocked` / `refused`; `retired`). Each
  attempt is `spa/src/core/deviceConnectionAttempts.js` (`idle`, `connecting`,
  `succeeded`, `failed`). The backoff is `spa/src/core/deviceRecovery.js`. The UI
  shows the projection from `spa/src/core/connectionStatusModel.js`
  (`connected`, `attempting`, `waiting`, `offline`). The existing dashed ring
  rotates during the post-connect sync active period reported by
  `subscribeConnectionSync`: final chained passes plus each device's bounded
  queue snapshot, excluding later traffic. Reduced motion disables rotation. This
  connection status exception does not expose sync state to other cache-rendered views.
  Per-device contexts live in
  `spa/src/core/deviceContexts.js`.

`sessionRpc.js` shares eight SQL sender credits across the opening's carriers
and priorities. Extra reads stay unsent and have no wire deadline yet.
Background SQL reads have a separate 12 s admission deadline, independent of
path timeout opt-outs. Expiry atomically removes the unsent reservation and
rejects with a retryable, definitely-unsent `deadline: "admission"` timeout;
it is no reason to probe the wire. Existing sync failure handling retains cached
content and recovery debt and releases the background stream slot. Foreground
SQL admission remains unchanged. Admission starts a fresh path deadline. Each
credit records its actual sending carrier and settles once on a final reply,
unsent cancellation, that carrier closing, authoritative restore, or opening
end. Receipts, timeouts, sent cancellation and switching carriers retain it.
Once the carrier's send is invoked, a rejection also retains the credit:
DataChannel sends can fail after writing chunks. Failures before that handoff
release the unsent reservation.
`connection.js` clears old SQL calls before restore greeting/resync, fenced by
the current lifetime. Affected promises receive retryable uncertain-delivery
errors; late replies cannot release replacement credits. Cache rendering and
read coordination remain independent of these admission waits.

`bridgeEpoch.js` accepts epoch/incarnation authority only from a current
greeting. Before the first greeting, authenticated `rtc.ice` pushes and
correlated bootstrap replies can reach the peer negotiation: its channels must
open before `session.hello` can run. Other pushes, including `rtc.diagnostics`,
retain the epoch guard. Once a greeting establishes authority, signaling also
uses that guard. Application calls await the greeting, and every later
app/terminal frame is fenced before correlation or push delivery. An unexpected UUID asks
for a fresh greeting; it does not establish a newer epoch. Replacing an epoch
settles unsent calls as `scope_ended` and handed-off calls as
`uncertain_delivery`, releases reservations once, and never replays terminal
input. `sessionRpc.js` routes durable acks and late confirmations separately
from transport receipts, including after the original caller timed out.

A surface calls the bridge through its device context: `context.rpc(method,
params)` from `contextFor(deviceId)` or `routeContext(route)`. `context.rpc`
refuses when that device cannot answer.

### Capability gating

- `hello()` in `spa/src/core/changeEvents.js` sends `session.hello` with the
  client's `api_range`. `greetBridge()` then selects an adapter and arms the
  change subscriptions.
- `selectAdapter()` in `spa/src/core/bridgeApi/index.js` checks the bridge's
  `api_version` against `SPA_API_RANGE` (`>=3.24.0 <4.0.0`). A greeting with
  no version, or no greeting at all, reads as `PRE_ALPHA_API_VERSION`
  (`0.0.0`), which is rejected. A version outside the range returns
  `{unsupported: "bridge" | "app"}`, and `spa/src/views/versionGate.js` says
  which side needs updating.
- `bridge.epoch` and the greeting's exact `ack_confirm_methods` select the
  durable write service. Every listed semantic method uses it; there is no
  `write.mode` or separate ack-confirm negotiation. Session controls are
  excluded. The SPA and bridge ship together; #739 rolls before the #740 SPA.
- `capabilitiesOf()` in `spa/src/core/bridgeApi/v1/index.js` turns the greeting's
  `capabilities` array into feature flags. Every bridge the adapter admits sends
  one; the minor-version table that stood in for it went with 1.x. New features
  get a name.
- Surfaces read the flags with `bridgeCapabilities(deviceId)`
  (`spa/src/core/changeEvents.js`), which falls back to `NO_CAPABILITIES`.
  Wire 3.21.0's `git.projectSources` and `changes.projectSources` are
  `git.projectSources` and `changes.projectSources` there. Project Changes reads
  its rendering support from the per-device `project-changes-support` cache
  record (`projectChangesSupport.js`) on cold mounts; greetings update that
  record. RPC and subscription dispatch still require the current greeting's
  corresponding capability. Without source Git support, the first source keeps
  the legacy `project_id` scope and additional Git sources explain the update
  requirement. No new source selectors go to an older bridge. Sync base uses
  `sources.syncBase`; its captured-path precondition additionally requires
  `sources.syncBasePath`.
- Conversation requests preserve their captured generation and exact agent,
  conversation and thread selectors. App dispatch waits for the current
  greeting and adapter; there is no pre-greeting legacy-shape conversion.
  `conversation.reset` still controls whether the reset action is offered.
- The task and conversation watch switches, task attachment buttons, and
  conversation compaction settings are available before a greeting (#182).
  Their values come from cached task and agent records. A late greeting does
  not replace the controls or their drafts, and an older bridge refusing a
  chosen command is explained in plain language (`commandRefusal.js`).
  Automatic task read marks still require watch support; a mounted task page
  resumes that housekeeping when its device greets.
- A flag that changes what a view draws is written to the cache at the
  greeting and read from there, so a cold mount draws what it will keep:
  `compaction.harnessThresholds` is persisted per device for compaction
  rendering. The adapter and cache service require the current greeting for
  provider-map writes and the new scope setters, including after downgrade;
  cached rendering support cannot authorize a request. A capable greeting
  enables a change subscription before refreshing authoritative policy and
  held agent records. Editors and menu changes belong to the next slice.
  `tasks.waitingOn` is persisted by `taskWaitingOnSupport.js` for task-page
  Waiting on and Blocks controls, together with the highest support seen on
  that device. `taskWaitingOnActions.js` waits for the current greeting before
  dependency writes, reverse reads, saved-id retries and workspace assignment.
  A capable current greeting admits assignment normally: the bridge enforces
  prerequisites. An incapable greeting admits legacy workspace assignment only
  if the history confirms the device has never advertised waiting-on support.
  Failed history reads/writes and unknown records fail closed in the page that
  observes them; shared in-memory capability evidence also refuses assignment
  in informed open tabs. Refusals include a retry or refresh hint alongside the
  update message. Evidence that never persisted has the page-lifetime residual
  described under Waiting-on capability history. A previously capable
  device is downgraded: board/list and task-page new-workspace pickers refuse
  with an inline update message and retain note/selection without sending.
  Even a ready task is refused on that device; legacy writes can erase graph
  evidence, so task replicas cannot safely narrow the guard. Unassignment and
  established non-workspace targets retain their request shapes. New dependency
  verbs, create-time edges and saved-id retries still require current support.
  After reading cached device history, the sender rechecks the same greeting
  authority and validates/sends without another wait. Rendering stays entirely
  cached. `taskWaitingOnSupport.test.js` covers monotonic history, greeting
  races, prior cached support and reload; `taskWaitingAssignmentDowngrade.test.js`
  retains all six round-8 legacy-erasure reproductions through the real picker.
  Successful responses update tracker detail and list replicas through
  `taskWaitingOnCache.js`. Edge add/remove requests another detail through the
  task page's trailing reader. Board/list assignment also requests a detail
  through `trackerTaskDetailsFeed.js`'s bounded trailing reader: a partial
  response must not leave an old timeline current at a newer task version.
  The mutation fence rejects reads begun before the response; the replacement
  read restores timeline history and unread mentions. Same-clock cache writes
  are distinguished by record identity. `trackerTaskChecklistDom.test.js` and
  `trackerAssignmentDetailRefresh.test.js` cover these respective paths.
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
| Workspace navigation | linked project ancestry, the local workspace selector and global project/workspace switcher in `spa/src/core/toolbar.js` (+ `toolbarModel.js`, `toolbarRender.js`, `toolbarBreadcrumbs.js`, `navigationModel.js`, `navigationRoutes.js`); the rail down the left edge (Changes, Files, Tasks, Settings) in `spa/src/core/workspaceRail.js` over `directoryRail.js`, mounted from `spa/src/views/workspaceView.js`; inside Changes, the per-directory tab row in `spa/src/views/workspaceChanges.js` |
| Project navigation | the rail down the left edge (Tasks, Changes, Files, Workspaces, Settings) in `spa/src/core/projectRail.js` over `directoryRail.js`, mounted from `spa/src/views/projectView.js` and `spa/src/views/trackerTaskView.js`; local return state in `spa/src/core/projectRailState.js` |
| Changes and git | `spa/src/core/gitPane.js`, `gitRender.js`, `changesReview.js`, `changesModel.js`, `changesRender.js`, mounted from `spa/src/views/projectChanges.js` (original configured sources), `spa/src/views/workspaceChanges.js` (named workspace directories) and `spa/src/views/branchView.js`; project and workspace directory panes are kept while another stands, and hide/show suspends owned editors, dialogs, listeners and layout work while preserving drafts, scroll and cache subscriptions |
| Files | `spa/src/views/files.js`, `spa/src/core/fileRoots.js` (a workspace's one root per directory), `spa/src/core/fileTree.js` (+ `fileTreeModel.js`), `spa/src/core/fileTabs.js` (+ `fileTabsModel.js`), `spa/src/core/fileViewer.js`, `spa/src/core/fileEditor.js` |
| Terminal | `spa/src/core/console.js`, `spa/src/terminal/` |
| Settings | `spa/src/views/settingsModal.js`, `settings.js`, `deviceSettings.js`, `devicePanels.js` in `spa/src/views/`; `spa/src/sheets/` |

The welcome screen and local Settings share the quiet Sign out link from
`spa/src/core/signOut.js`. In a browser it leaves the document for the server's
`/auth/logout` confirmation; Electron intercepts that exact same-origin path
in `desktop/src/security-policy.mjs` and invokes the Account menu's existing
credential and storage cleanup in `desktop/src/main.mjs`.

Add project (`spa/src/sheets/newRepo.js`) replaces its form with the quiet
source list in `newRepoProgress.js` while `project.create` runs. Creation's
internal pending row is not published and the bridge has no per-source
creation events, so all lines run and settle together. Escape and scrim
dismissal are blocked only until the request settles. Failure keeps the
source errors visible; Back restores the draft and focus to Create. Transport
loss or either RPC deadline shows an unknown outcome and Close, explaining
that the project may still be being created and will appear in the Projects
list if it succeeds. The existing transport-error predicate distinguishes
these from bridge refusals, including uncoded ones. A stable live summary is
outside the busy actions and atomic announcements exclude the source list. Success
writes and rereads the project cache before opening it. Dismissing the settled
sheet prevents a delayed cache write from navigating afterwards. An opening
or cache failure after successful creation keeps source lines Done and reports
the opening error separately without repainting the unchanged source list or
offering another creation attempt. The settled success action is Close.

Agent name badges use the family rule beside `shortModelName` in
`spa/src/core/agentChoice.js` and the shared painter in `agentModelBadge.js`.
They describe the current cached roster model (active model before the next
choice); task and message identities do not record the model at write time.
The family is dim monospace, with the short versioned name and known effort in
its native tooltip. Unknown model families leave no badge. Task headers and
agent-message attribution read the already held feed, and changes to cached
roster models or efforts also invalidate the thread paint.

Usage limits remain cached per device in `spa/src/core/usageLimits.js`. The
conversation banner filters those records by the selected agent's provider
family from `providerCatalog.js`, so Claude headless and TUI carriers share
their limits without displaying them on Codex sessions. Rail switches reread
the provider in focus; the overview and new-agent chooser show no session limit.

Unread counters count only watched conversations and tasks. A workspace's
count sums its watched agents. Conversation watches persist a thread sequence
baseline on the agent; task watches persist a timeline entry baseline
(`watch_started_after` on task records and wire summaries). Counts
start strictly after the later of that baseline and the user's read cursor.
Repeated watch requests retain the baseline; unwatching and rewatching starts a
new baseline. User-created conversations watched from creation start at zero.
Automatic task watches exclude old history while retaining the triggering news.
Conversation reset returns its watch baseline to zero for the new history.
The SPA also treats cached `watched: false` as zero on older bridges, without
waiting for a greeting. Wire 3.19.0 announces `unread.watchScoped`; older
bridges cannot supply watch-start-scoped counts.
The project Tasks badge follows both cached list and detail records, using the
same unread calculation as Inbox, so an accepted task-page read clears both
before the next list refresh. The task page's New divider also starts after
the watch baseline (`watch_started_after`, or the legacy watch event).

Inbox rows, including Recent and the project Workspaces tab, use
`spa/src/core/inboxStatusDot.js` for one right-edge status dot. Unread activity
is green; running activity without unread is grey. Only running agents pulse,
and reduced-motion preferences disable the animation. Watched tasks read their
assigned agent's running state from cached rosters. A folded project head
summarizes watched activity, including tasks without a Needs you reason; an
expanded head leaves status on the project agent’s own row.
When only descendant news lights a coordinator's dot, its tooltip says
"A worker has unread messages"; its own unread retains "Unread activity".
The head offers Hide and New workspace directly.

The Projects face lists every project in one list ordered by the oldest
current streak start among its active watched entries, including the project's
own conversation. Projects with only Recent rows or no dated activity follow
active projects; case-insensitive name, device-tag and project-key collation
breaks ties. The toolbar's project picker retains alphabetical order.
Project blocks have transparent backgrounds; only the active head has an accent
tint. Every block starts open unless the user saved a fold, with its live rows
in anchor order followed by one project-keyed Recent disclosure. Recent starts
closed with a quiet-row count, even when the block has only quiet rows; opening
it paints quiet workspaces and project agents on one compact line, with facts
floating to the right of the name; watched tasks retain title and reason
lines. The project agent is an ordinary entry inside its block, using the same
order and Recent cutoff as its workspace siblings. The block name and chevron only fold the
whole block independently of its saved Recent state. Projects never enter a
top-level Recent. The ungrouped Inbox face keeps its own Recent disclosure.
Every Inbox workspace, project-agent and task entry, including Recent, has a
separate project line above its existing content. Captures with a known project
also gain that line. Inbox Recent keeps its 13px indent and compact workspace
and project-agent line. Projects omits the project line because the block
names the project.
Project-agent names use their cached topic when present, otherwise the existing
project label. Project-agent activity uses its own cached conversation’s
`entryFactsText` pending/stat facts, with Getting started when empty, then the
workspace running-count treatment when a roster is known. Quiet tasks still
omit the activity line when it has no text. Existing row actions remain
unchanged. The project-agent row is a focusable link; Enter opens the same project destination as a click while the
heading only folds.

Inbox task rows come from cached `tasks.list` / `tasks.get` records through
`spa/src/core/watchedTaskFollower.js` and `watchedTaskRows.js`.
Project task lists refresh independently of device ownership and Needs-you
policy (`watchedTaskDependencies.js`). Row hints share a finite device pass
with at most one trailing pass queued; they do not reread every project's task
list or policy.
Unchanged derived state publishes no redraw. A project's first publication
waits for its cached task list, agent ownership and actual policy, including a
false policy. Later reads retain per-dependency eviction and session fences;
user RPCs and cache writes do not acquire these background read slots.
An inbox task opens in its assigned available agent's workspace Tasks tab with
that agent's conversation, falling back to the project task page when no cached
live workspace is known.
Every watched open task outside Done stays visible, even after its attention is read. Rows
interleave with captures, workspaces and project-agent rows in one list ordered
by oldest current activity-streak start (#628). A gap of eight hours or longer
ends a streak. Conversation summaries come from the bridge's message index;
task summaries combine meaningful timeline entries with durable content-edit
checkpoints and are cached on `tasks.list` / `tasks.get`, so sorting needs no
history fetch. The newer cached task copy supplies activity when detail and
list arrive independently; the list settles equal update stamps. Older task records
fall back to their anchor or creation time and latest cached activity. Attention
reasons and unread counts never change position. Every row kind moves
to Recent after a day without activity while no agent works; dismissed rows go
there too. Inside a project block, the same row order and Recent rule apply.
`inboxOrdering.js` holds that policy separately from row markup. Cache writes
reconcile both faces immediately, including while an inbox input is focused;
the keyed rows preserve the input and selection. An active capture with a
focused branch editor keeps its active partition past the cutoff until focus
leaves that editor, including its Dispatch button;
ordering still updates, and a focused editor already in Recent stays there.
Branch drafts are device/capture/destination-project scoped `build-ui` records,
flushed on teardown and restored when the row or field remounts, including
after moving into Recent. A local cutoff clock repaints
when the next cached row reaches 24 hours and on page return, without polling
the bridge, and is disposed with the inbox.
Quiet tasks keep their title and reason on separate lines. Every task row has an
eye that uses the existing optimistic `tasks.unwatch` path, removing the row
immediately and restoring its watch if the write fails. The eye stays visible
on touch screens. Needs you reasons and unread counting retain their own rules.

The inbox and agent rail share `agentIsRunning` in
`spa/src/core/agentRunning.js`: an agent runs while its own loop or an agent
in its activity panel runs. The task feed derives those descendant rollups
from the freshest cached rosters, per device and project, using the cached
`agents.createdBy` capability for Build descendants. The agent's own
`working` flag stays separate for interruption controls.
The feed also derives descendant unread from that canonical lineage (#704).
Only watched conversations supply news; project and workspace ancestor dots
summarize it with an ephemeral `agents_unread` flag. Their own unread counts
remain unchanged, so a message contributes to the Inbox total only at its
owning row. Complete project rosters use the same lineage, including aliases,
and a cached read update clears ancestor dots without a history fetch.

### Window chrome

The desktop shell's macOS title bar overlays the app's first 44px row.
`core/windowControlsOverlay.js`, installed before boot, reflects the standard
`navigator.windowControlsOverlay` geometry into layout only; it exposes no
Electron bridge. `styles/window-controls.css` clears the native traffic lights
in the pinned inbox header or the main toolbar, follows the responsive overlay
breakpoint, and marks empty headers draggable while excluding interactive
targets and menus. An absent/inactive overlay (browser, standalone PWA or native
fullscreen) keeps the ordinary shell layout. Linux/Windows retain native chrome.

Development builds (`VITE_BUILD_CHANNEL=dev`) mark the connection button with a
compact “D” corner bubble and the accessible description “Dev build”.
`core/buildChannel.js` sets the tab title before boot and decorates the button
when `connectionStatus.js` creates it; connection transitions retain that node.
Production builds have neither the bubble nor its description.

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
