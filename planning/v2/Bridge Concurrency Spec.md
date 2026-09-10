# Bridge Concurrency Spec

Status: approved 2026-09-04. Branch: `fix/bridge-concurrent-requests`.

## Problem

With three or more agents working across branches, the bridge slows to the
point that ordinary requests (`thread.post`, `worktree.create`,
`branch.dispatch`, `board.list`) exceed the browser's 12 second RPC timer
(`spa/src/core/session.js`, `DEFAULT_RPC_TIMEOUT_MS`).

One `std::sync::Mutex<AppState>` (`bridge/src/app.rs`, `AppState::shared`)
guards every project, run, plan, terminal tab and screen, diff cache, and the
SQLite store. All eight relay dispatch workers (`relay.rs`, `DISPATCH_WORKERS`)
run frame handlers on `spawn_blocking` and each takes that mutex. Prior work
(`DeferredWork::{Read,Git,Finish}`, `warm_diff_caches`, `spawn_diff_refresh`)
already moved most git verbs, worktree finish, and diff refreshes off the
lock. Five paths still serialize the daemon.

## Findings, ranked by impact

1. **Terminal output is parsed under the global lock.** `spawn_tab_pump` takes
   the app mutex for every PTY chunk from every agent and runs the vt100
   parser inside it. Contention scales with the number of streaming agents.
2. **Worktree creation runs `git worktree add` under the lock.**
   `worktree.create`, `branch.dispatch` (`cut_branch_for_dispatch`),
   `run.create` / `issue.implement_*` (`ensure_issue_implementation_worktree`,
   `open_implementation_run`), planning worktrees for `plan.create`,
   `run.abandon` / `run.adopt` / `run.release` (discard or claim a checkout),
   `project.add` / `project.clone`, and `undo_branch_dispatch` all shell out
   to git while holding the mutex.
3. **The reply to a message waits for the agent to spawn.** `dispatch_frame`
   calls `deliver_pending_agent_turns` before answering. A cold spawn waits
   for harness readiness up to `HARNESS_READY_GRACE` (20 s); the browser gives
   up at 12 s. The message was durable, only the reply was late.
   `ensure_agent_tab` also runs the resume/transcript probes and
   `scaffold_agent_worktree` under the lock, and spins 25 ms sleeps up to
   `AGENT_SPAWN_WAIT` (30 s) when another spawn for the same tab is in flight.
4. **`board.list` computes under the lock on a cache miss.**
   `external_worktrees` falls through to `discover_external_worktrees` (status
   plus two diffs for every worktree of the repo) when the cache is empty.
   `invalidate_external_scan` empties it after every worktree create.
   `warm_diff_caches` waits `FIRST_COMPUTE_WAIT` (5 s) per key, sequentially,
   then the route computes under the lock anyway. `run_stat` computes on miss
   the same way.
5. **`term.input` writes to the PTY under the lock.** A harness that stops
   draining its pty blocks the write and wedges the daemon.

Not the problem: SQLite (WAL, `synchronous = NORMAL`, sub-millisecond writes),
the idle monitor and terminal reaper (short holds), the change bus (its own
leaf lock).

## Design

### Principle

The app mutex protects in-memory bookkeeping only. Nothing that can block on
a process, a pipe, a socket, a filesystem walk, or a sleep runs while it is
held. A request answers as soon as its own state change is durable; work it
triggers runs on a background task and reports through push invalidation
(`ChangeBus`) and the entity's recorded state, never through the reply.

### Step 0: frame timing

`relay::run_job` and `dispatch_frame` record queue wait, lock wait, hold time,
and total per frame. Any frame over 200 ms logs one line with method, the
four durations, and the number of frames waiting. A `bridge.stats` verb
returns counters (frames served, p50/p95/max per method since boot, current
lock holder method, current queue depth). This ships first so every later
step has a before/after number.

### Step 1: terminal I/O off the app lock

Each tab's screen lives behind its own `Arc<Mutex<TerminalScreen>>`
(`Tab.screen`). The output pump holds a clone and never touches `AppState`
after start. `term.attach`, `agent.attach`, snapshots, `term.ack`, resize, and
close read and write the screen through that handle. The app mutex is taken
only to look the handle up.

`term.input` and `term.resize` clone the session handle under the lock,
release it, then write. The PTY writer is unchanged.

### Step 2: reply and delivery split

`deliver_pending_agent_turns` runs on a spawned blocking task, not on the
frame's worker. A verb that queues turns answers as soon as its store write
lands. The queued turn's owner is marked in flight under the same lock
acquisition that takes the queue, exactly as today, so the idle sweep never
sees a gap.

Delivery outcome reaches the browser through what already exists:
`record_agent_session_start` / `record_agent_delivery_failure` plus
`note_entity_changed`. Verbs whose reply embeds the spawned tab
(`agent.start`, `agent.choose`, any reply carrying `term_id` or `wire_id`)
return the reserved tab id from the registry entry created under the lock;
the pump attaches when the session opens. The SPA renders "agent starting"
from the entity's push event, not from the reply.

`ensure_agent_tab` moves the resume/transcript probes, the session locator,
and `scaffold_agent_worktree` off the lock: read the inputs under the lock,
release, probe and scaffold, re-acquire to insert. The in-flight wait becomes
a `Condvar` or a `tokio::sync::Notify`, not a sleep loop.

### Step 3: board reads never compute under the lock

A read that finds no cache entry returns what it has (empty external scan
with `"scanning": true`, `stat: null`) and triggers the refresh. The refresh
publishes through `publish_diff_refresh` and `note_board_changed`, and the
browser refetches. `FIRST_COMPUTE_WAIT` and `wait_for_first_diff_value` are
deleted. The `claim.blocking` path is deleted; every refresh is spawned.

A worktree create inserts its new worktree into the project's scan cache
(`ExternalScanCache.worktrees`) instead of clearing the cache. Removal deletes
the entry. The periodic refresh reconciles the rest.

### Step 4: worktree lifecycle off the lock

Every verb in finding 2 becomes a two-phase `DeferredWork` like
`worktree.finish` already is:

- **decide** (under the lock): validate, reserve the name and the entity
  record in a `Creating`/`Discarding` state, persist that state, return the
  job.
- **run** (lock released): `git worktree add` / remove, scaffold, probes.
- **apply** (under the lock): write the result back, stamp interaction,
  note changes; on failure roll the reservation back with the existing
  `undo_branch_dispatch` / `BranchDispatchCreations` logic.

`WorktreeLifecycleJob` is one primitive shared by create, dispatch, implement,
adopt, release, abandon, clone, and rollback. The board shows a `Creating`
entity immediately; the SPA's optimistic overlay already covers that row.

### Step 5: SPA

`session.js` keeps the 12 s timer. Views that waited for a tab id in a reply
(`thread.post`, `agent.start`, `branch.dispatch`, `worktree.create`) read the
entity's `agent`/`worktree` fields from the next push instead. The optimistic
overlay (`core/optimistic.js`) gets an `agent_starting` state for the row.

## Verification

- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- A test per step that holds the relevant off-lock phase open (the existing
  `OffLockGate` pattern) and proves a second frame completes meanwhile.
- A load test in `bridge/tests/`: three PTY sessions streaming output while
  `board.list`, `thread.post`, and `worktree.create` frames are timed; p95
  under 200 ms for the reads, replies for the writes under 500 ms.
- Manual: three agents working, `bridge.stats` shows no frame over 1 s.

## Out of scope

Per-project state partitioning. After steps 1 to 4 the only work under the
lock is in-memory; revisit only if `bridge.stats` still shows lock waits over
100 ms.

## Rollout

Merge to `main`, deploy, roll the local bridge (`cargo install` + kickstart).
Never deploy from this branch.
