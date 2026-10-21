# Bridge Concurrency Plan

Task #681 — revision 5, amended by #710, 2026-10-20. **Planning only.**
Amendment baseline: `82b5ce0c47b3016fb237cd0a2db5376384d019c2`, wire 3.23.0.
Steps 1–3 are on main; #694 is finishing step 4 and #709 is reducing write-path
hold time. Their completion is not claimed here. Original inventory/timing baseline:
`8257cba8bc737b622978ccda3c20e43b7feb9222`; incorporates reviews #682/#683. Paths below start at
`bridge/src/` unless stated otherwise; app-area paths start at `bridge/src/app/`. New modules/functions
are marked proposed. The [architecture amendment](../../ARCHITECTURE.md#app-state-and-its-lock) distinguishes this design from current behavior.

The [Write Acknowledgement Protocol](<Write Acknowledgement Protocol.md>)
defines durable write ack/confirmation, bridge epochs, MCP outcomes, cache
reconciliation and the three scope decisions Zech accepted on 2026-10-20
at 19:08 UTC (recorded on #710): real-time/session controls are exempt from
durable early ack, reads target p99 ≤100 ms for bounded complete results plus
max ≤100 ms in controlled lock-isolation tests, and ack durability covers a
bridge-process restart with WAL/NORMAL; power-loss durability is not required.
**Ack means UI success; confirmation means canonical application and publication.** It supersedes
the original plan's use of “ack” for a completed write. Reads never wait for
an accepted command to finish. The amended sequence is:

| Step | Shippable result |
| --- | --- |
| 1–3 (shipped) | Comparable timings, independent SQL reads, immutable snapshots and safe publication. |
| 4 (#694) | Independent observation I/O; complete read dispatch avoids app-lock admission/apply/drain too. |
| 5a (#709), then 5b | Cut write hold time first; remove global finisher maintenance, then establish universal compatibility ownership. Do not make 5a wait for the journal. |
| 6a, then 6b | One short SQL writer boundary and durable command/epoch foundation, then ack/confirm on `mcp.control` hot writes, internal `agent.deliver`, `thread.post`, `entity.seen`, `user.present`, `tasks.comment` and `tasks.update`; persistent SPA overlays and recovery ship together. |
| 7 | Extend ack/confirm and granular ownership to remaining task/conversation, lifecycle, filesystem, Git, config and review writes. |
| 8 | Complete session/autonomous ownership, MCP action/report coverage and remaining bounded-read/cache contracts. |
| 9 | Remove compatibility paths; prove the ≤100 ms read gate, fast durable acks and recovery contract. |

## 1. Contract and measurement

SQLite reads never acquire an app/domain/writer lock or share write/Git workers. Snapshot reads
load an immutable value without a reader lock. **Neither read kind returns `busy` for capacity.**
SQL readers may wait for their own read connection, never the app lock or writer; storage/CPU still cost time.
Independent workspace writes overlap except for short SQLite commits or a shared physical Git
repository. A read issued after a successful **confirmation** sees that write
or a later revision. After durable **ack**, canonical reads may still see the
previous committed view; the originating client's effective cache retains
its accepted overlay. Read-after-ack never waits behind execution.

At the original pre-step-1 baseline, `carrier.rs::FrameIntake::accept` fed a per-wire admitter (1,024), then
`carrier/dispatch.rs::Dispatcher` (two queues of 256, eight workers, at most two background).
`run_handler` uses `spawn_blocking`; a deferred Git job frees the app mutex but keeps its dispatch
worker. Terminal lanes, immediate signaling, MCP and services are separate paths.
`app/rpc.rs::routed` takes `Mutex<AppState>`; `api/v1/mod.rs::Handler::call` receives `&mut AppState`.
`Store` clones share one `Arc<Mutex<Connection>>`; WAL is enabled, but reads and writes
serialize on that connection. `runtime/off_the_workers.rs` protects Tokio workers; it does not
remove either serialization point. Steps 1–3 have since separated six SQL
reads, nine intake snapshots and stats; current behavior is in
`ARCHITECTURE.md`. The table below records original measurement gaps,
not a claim that those shipped changes are missing.

| Evidence | What it establishes / what is missing |
| --- | --- |
| `timing.rs::QueuedFrame`, `FrameTimer::lock`, `LockedFor` | Dispatch-to-handler queue time and instrumented app wait/hold; queue includes blocking-pool scheduling but misses per-wire admission. |
| `FrameClock::stats`, `MethodRecord::stats` / `bridge.stats` | Per-method total/hold distributions and served counts; queue `p95_ms` is total frame duration, not queue-wait p95. Stats itself currently queues. |
| `slow_frame_line` | Total, queued, lock_wait, held; throttled at one/method/10 s over 200 ms. It is not a complete trace or a verb-frequency sample. |
| `Store::connection`, `in_transaction` | No separate SQLite duration. SQL is included in app hold when invoked under the app lock; its share cannot be inferred by subtracting percentiles. |
| Focused existing timing/dispatcher tests | Demonstrate synthetic queue and lock waits, ordering and folding, not production verb dominance. Folded served counts count computations, not callers. |

Step 1 adds only per-verb queue-wait, lock-wait and hold counts/sums/histograms to the existing clock.
Carry the intake timestamp through admission for comparable intervals; preserve stats keys and logs.
Registered methods need bounded records (current cap 128 is below 138 v1 names); collapse unknowns.
No paths/content labels, new fixture framework, ten-phase tracer or scope metrics.

Use existing timing/dispatcher tests plus a temporary file-backed Store fixture. Record revision,
verb, count and queue/wait/hold p50/p95/max and sums, isolated and during gated write/Git work;
compare after steps 2/3. Extend fixtures only for each step's deterministic interleavings.
Rank counts/summed wait/hold to adjust later priorities, without delaying shipment. Thread/tasks
then board follows the assignment, not measured frequency. SQLite cost and dominant production
verbs remain unknown; #672's reported 6 s → 0.4 s board improvement is context, not reproduced here.

## 2. Read execution and publication

### SQL lane first

Capture service handles in `AppState::handler` outside request locking. Add a small immutable
`ReadContext`: registered project ID/path mapping, admitted live owner/agent identities,
deletion/reset generations and required task identity/user-session overlays, including cached
canonical root/workspace links. Startup and lifecycle transitions update it before replies.
Durable identities advance only after save success; live availability reflects actual retirement
even on an existing partial-failure path, never failed durable roster fields. Configured project
IDs are persisted/restored despite the stale boot-local comment in `tracker/views.rs`; persisted
tasks are keyed by project path.

Start with **eight read-only WAL connections and eight dedicated SQL read workers**, independent
of write/Git blocking workers. This is an initial engineering size, not a measured optimum; tune
from checkout wait and query duration in the step-2 comparison. Open them after the existing
writer's startup migration, with query-only protection. A connection has one borrower; one request
transaction supplies all related rows and is ended before return. When all eight are in use, await
a fair pool permit asynchronously and reuse the next returned connection. Never fall back to the
writer connection, return a capacity error, or hold an app guard while waiting. Bound query pages
and release cursors promptly so ordinary pool waits stay short; arbitrary stalled queries cannot
be promised a fixed wait bound. Cancellation removes the waiter. SQLite/storage errors remain real
errors, not capacity refusals. Preserve unbounded `tasks.list` when no limit is supplied; use
internal batching where compatible and measure its transaction duration, not a new wire page cap.

Use the existing single writer during this first read migration; the writer worker is step
6. Thread binding, alias resolution and generation validation must use the same SQL
transaction as history/operation rows. Existing `agents` records contain conversation/thread
skeletons; reuse them and reset's atomic metadata/history update, adding SQL count/sequence
metadata omitted from the serialized skeleton. Audit legacy identity materialization and
required summary fields before switching each query; only add schema if a demonstrated
lookup is missing. Keep the admitted-owner filter so retained SQL entities do not become
newly addressable. Preserve bounded pages, newest-only activity, exact digests and
Store-free immutable fallback behavior.

`tasks.list/get` combine transactional durable task/timeline/dependency rows with the small
identity/user-session overlay; they are **S+M**, not falsely SQL-only. Move identity/done-at
backfills and cursor-key creation to startup or background work; pure derived fallback fields
retain legacy output until backfill completes. Do not mutate through refusal formatting in
`api/v1/tasks.rs`. Live observations may lag in-flight work but must match the canonical identity
and generation. No old SQL transaction survives a pool return.

### Snapshot lane and failed-save boundary

Publish immutable `ReadView` chunks for board/project/workspace/agent/terminal rows and service
facts. Use an atomic Arc load, not one replacement global RwLock. Prepare projections outside a
short root update; merge disjoint versioned deltas into the latest root, retain deletion
generations, and apply multi-owner changes together. No read refreshes caches, walks Git, queries
Store or canonicalizes a filesystem path. Do not clone entire histories.

Before a family moves, every producer of its canonical fields must publish successful changes
before confirmation (the legacy completed-write reply). At the original
baseline `finish_plan_mutation` and `finish_run_mutation` save, then install
changed objects and emit news even when persistence failed. Fix both in step 3: stage durable
fields/attention and the candidate projection; promote them and emit change/news only after save
success. Keep the last committed canonical fields separate from live process handles, so a failed
candidate cannot leak through a later unrelated refresh. Runtime retirement already performed need
not be undone, but cannot be mistaken for a successful canonical save. Audit `answer_*_mutation`
and caller cleanup, especially `agent_remove`; do not blindly clone/restore PTYs. Add a
failed-save → unrelated-success/refresh test for **each** finisher.

Move parked-project revival, workspace adoption/manifest refresh, board attention/cache/sweep work
and session-summary accounting to lifecycle or background producers. `session_seen` tracks
observed message sequences, not the user's read cursor; explicit seen/read-through/presence remain
writes. For `agent.list`, precompute canonical root, owner/workspace/tab identity, compaction
context and unread summary when owners/rosters/messages change; its current helpers reach both
filesystem canonicalization and stored unread history. Warm required observations before first
publication without inventing loading states or replacing unavailable canonical data with an empty
board.

### Admission, read-your-own-write and pushes

At `carrier.rs::FrameIntake::accept`, classify decoded frames before SQL/write admission. Answer
migrated snapshot reads directly from the immutable view and handle control frames (ping, close
and existing signaling fast paths) at intake, without a SQL permit, worker queue or app lock.
Stats follows this fast path. Preserve ordered terminal effects; I/O reads have their own lane.

Use a backlog of B=16 waiting SQL reads per session opening, in addition to the eight global
running reads. Count every accepted request waiting for execution/connection, even if moved
between internal queues. Use a fixed worker set and one ready marker per opening, not a spawned
semaphore waiter per frame. The SPA initially sends at most N=8 SQL reads per opening (N≤B),
shared across all carriers and priorities; further SQL calls remain unsent. Fixed defaults avoid a
new greeting negotiation; test the method allowlist against bridge SQL routing. Older/unpaced
clients use the safety net below.

`spa/src/core/sessionRpc.js::createSessionRpc` reserves credit before send, independently of
caller promises, and records the actual sending carrier. Return it on the **first** final reply,
**unsent** cancellation, that carrier closing, `onRestored` (clear the whole ledger), or opening end. Keep at
most N current reservations/sent entries; receipts, bare timeouts and a carrier switch alone do
not release credit. Cancel unsent entries immediately. Cancelling an already-sent read retains its
credit until a reply or carrier/opening event: no server cancellation
acknowledgment exists today (the #683 scope correction implemented by #685). Late replies cannot release a
replacement credit. Start wire deadlines after credit admission; cache rendering remains unchanged.

Carrier close/restore reject affected SQL promises via `uncertainDeliveryError` with `retryable: true`;
API normalization/transient-read handling treats these like timeouts. In `connection.js::landSession`,
authoritative `link.onRestored` rejects/clears old SQL entries before greeting/resync, fenced by
`lifetime.current()`. Retain session identity/non-SQL calls; next pulls refresh cache. No reply rerouting.

On carrier release, prune that carrier's not-started SQL callers/jobs and free their backlog slots;
retain other carriers' callers in shared folds. Running SQL finishes on its connection, but its
reply to the released carrier is discarded. B=16≥2N covers one old/new batch overlap, including
`onRestored` without an actual process restart, so that recovery does not overflow the backlog.
Repeated resets/cancellations before old work drains can still reach the bounded safety net below.

Excess SQL suspends ordered intake: one parked intake/carrier, no read `busy`. Admitted snapshot/control
frames stay independent; later unread frames on that carrier may wait (`rtc.rs::pump_channel_events`
awaits `RidingChannel::accept`). Carrier release/opening end clears queued work/parked intake via a
close signal independent of blocked polling. **Opening end conditions stay unchanged:** closing A
after B attaches keeps B usable. Only writes retain capacity refusals.

For synchronous/legacy callers, order successful mutations **commit → publish
affected canonical views → stage change events → final reply**. For negotiated
ack/confirm, add **validate → persist intent → durable ack** before execution;
the completed path is **commit applied marker + canonical SQL → publish
affected views → persist terminal outcome/outbox → confirmation**. Prebuild
the projection before committing; the in-process publish operation applies
an owned delta without fallible I/O. Keep that short completion job alive after client disconnect.
Preserve domain receipts/recovery contracts and link them to the new mutation
journal. A command ID is not permission to replay an ambiguous external
effect. Restart rebuilds committed projections before advertising readiness;
no generic live failure callback rewrites history. Durable ack resolves the
UI action through an accepted cache overlay, not fabricated canonical
completion. Transport receipts are not durable mutation acknowledgements.

A post-confirmation snapshot load or fresh SQL transaction sees the write.
Already in-flight responses need generation/revision fences at the cache.
Fold only compatible queued reads; never attach a post-confirmation caller to an older running
read. Carry `SessionSender` and opening generation explicitly across workers. Preserve terminal
verb order and close/reopen fences; an arbitrary `term_id` must not route a SQL read through a
terminal lane. Push facts use committed projections/readonly SQL, outside bus locks; the existing
250 ms coalescing and existing change payload shapes stay; the new negotiated
epoch envelope and confirmation stream are separate. Failed-save finishers emit no change/news and publish no
failed canonical candidate. Independently completed live retirement and best-effort user-session
observations retain their existing semantics, with separate tests: they must never carry failed
durable fields. Network delivery does not delay the reply.

## 3. Write ownership and lock order

Resolve owners from canonical server identities, not unchecked paths. Project conversations/source
terminals are project-owned; uploads recover scope from their session-owned admission record.
Shared config has its own short serialization service; shared physical Git refs use canonical
common-dir keys.

1. Reserve the complete **device → project → workspace** set, sorted by ID
   within a level, all-or-none. Child reservations share ancestor intentions:
   workspace A and B may run together; project deletion excludes its children.
   No upgrades or partial-set waits; discover another owner, release/recompute.
2. Lock owner records only to validate/capture owned inputs or apply results.
   Drop every state guard before SQL, filesystem/process work or publication.
   Keep the operation's conflict reservation through commit/publication so
   same-owner writes cannot publish backwards. Readers never reserve scopes.
3. Acquire sorted canonical Git exclusions after scope reservation, outside
   state guards. Remove `git/deferred.rs::ScopedGitCall::run`'s SyncLock→AppState
   reentry; release ordinary Git locks before applying state. No callbacks
   from leaf services into state/scope acquisition while holding their guards.
4. The SQL writer owns one writable connection, accepts typed owned commands,
   validates versions in the transaction, and returns committed data. It never
   acquires application/scope/native leases or calls app publication. Ordinary
   commands run no Git; the existing final-ref fence below is the narrow exception.
   No app/domain mutex is held enqueueing, waiting for, or executing SQL.
   SQL-only writes whose invariants are transactional need no application scope.
5. **Existing review exclusions:** acquire CaptureJournal after scope ownership;
   retain the existing operation's working/receiver/packed-ref fences before SQL.
   These narrow external leases may span SQL without an app guard.
   CaptureJournal is retained because it prevents two captures claiming the
   same task and preserves existing interrupted-capture cleanup. Receiver-ref
   locks span the metadata commit because releasing them would let received
   heads/pins change between validation and commit. Release both before the
   publication callback; the SQL worker never waits to acquire either lease.
   Retain `store/reviews/pull_requests/sync.rs::with_review_publication_version`
   as a typed special writer command: its final ref update inside the checked
   transaction prevents a concurrent review-version change authorizing stale
   publication; it cannot call Store or app/scope code. Preserve its existing
   fence tests rather than generalizing callbacks or moving validation earlier.
   Current explicit push also acquires tracking-ref locks *inside* SQL via
   `reviews/publication/tracking.rs::advance` and comparison helpers. Before
   routing that path to StoreWriter, pre-acquire its tracking lease after the
   existing outer fences and pass held-lease helpers into the final checked
   update. Audit aliases/cross-operation order; a global native-lock order is
   not established by the code. Test a held tracking lock leaves the SQL writer
   and WAL readers free, and retain receiver-pin/version-fence regressions.
6. Publish, persist/stage the terminal outcome and notifications, then release
   operation ownership and confirm (or send the legacy final reply).
   Admission/ack does not release an executing command's conflict reservation.
   No blocking state guard crosses `.await`; config/bus/screen/metrics internals
   are leaf services. Existing reset, source, receipt and instance fences remain.

Before enabling scoped writers, every legacy writer takes a conservative exclusive compatibility
reservation **before its first mutation**, including direct Store background paths. It conflicts
with migrated writer scopes but never reads. Narrow a family only after auditing its full helper
footprint. Global reaping must leave finishers before common writes claim sibling progress.
Snapshot/read lanes can ship while writers still use this conservative gate.

## 4. Implementation steps and tests

Each step is a release; write tests first and observe failure. Add **no complexity-ratchet entries**:
split new functions over Clippy's cognitive-complexity cap 15, never annotate them. Check
`bridge/tests/complexity_ratchet.rs`, Clippy and fmt under `nice -n 10`; judge actual exit status.
Tests use temporary paths/injected services; audit wider fixtures first. No
installed bridge, live daemon, protected homes/env or install scripts.
Ack/cache slices add isolated browser fixtures with fake transport and
temporary profiles; they never connect to a user's bridge.

### Step 1 — Minimal comparable timings

Change `timing.rs::{QueuedFrame,FrameTimer,MethodRecord,FrameClock,LockedFor}`,
`carrier.rs::{FrameIntake::accept,admit_in_order}` and dispatch timer plumbing in
`carrier/dispatch.rs::{run_handler,run_folded_read}`. Add §1's aggregates/intake timestamp;
preserve stats. Extend `timing::tests`/`dispatcher_tests` for gated queue/lock waits, counts,
registered-method coverage and unchanged slow logs. Capture the small before sample.

### Step 2 — Independent SQL reads, first user-visible improvement

Add proposed `store/read_pool.rs::{ReadPool,with_read_transaction}` and `app/read_context.rs`. Change `store.rs::new` and
`store/{tracker,conversations,operations,conversation_reset}.rs` to accept explicit readonly
transactions. Keep the existing single writer for now. Refactor
`store/entities.rs::{read_agent_records,read_agents,run_census,materialize_agent_identity}` for
readonly skeleton/metadata/legacy projection. Change `api/v1/mod.rs::{Handler,dispatch}`, v1
tasks/thread tables, `app/rpc.rs::{AppState::handler,dispatch_frame,session_scoped,routed}` and
`carrier{.rs,/dispatch.rs}` admission/executors: `SessionSender`/`Admitted` carry carrier identity;
`FrameIntake::{close_carrier,close_session,close_ended}` prunes queued SQL/empty ready markers;
the proposed SQL executor fences start/reply against carrier release. Preserve
`SessionRegistry::{release_carrier,release_session}` end conditions; change
`rtc.rs::pump_channel_events` close signaling. Read dispatch owns its result/update/deletion check
through captured services, not `deferred_result_check`. Proposed `api/effects.rs` covers all entries. Migrate
`tracker/mod.rs::{tasks_list,listed_rows,tasks_get,tracker_task,backfill_done_at}`,
`tracker/identities.rs::{backfill_task_identities,task_with_read_identities}`, `tracker/pages.rs`,
`tracker/waiting_on_api.rs::tasks_waiting_on_blocks`, `store/tracker.rs::tracker_list_key`,
`conversations/read.rs::{thread_page,thread_activity,thread_page_at,thread_page_after,stored_thread_page,
stored_thread_delta,lean_thread_page}` and `conversations/post.rs::thread_operation`. Update
canonical identity publishers after save success and live availability after actual transitions;
retain the agent-removal failure semantics above. Tests: SQL reads finish with app lock/write
workers/WAL writer held; two readers overlap and a ninth waits for its own pool without refusal.
Pin readonly enforcement, fresh transactions, zero read-path writes, cursor/backfill parity,
owner/alias/reset isolation, bounded history and post-confirmation visibility across connections; admission tests follow.

SPA: change `spa/src/core/sessionRpc.js::{createSessionRpc,call,takeEnvelope,createDeadline,
readFrom,rideOn,close}` for carrier-tagged, exactly-once credit release; proposed `sqlReadCredits.js`
and `resetSqlReads` keep functions within cap 10 without ratchet entries. Expose the reset via
`spa/src/core/session.js::openSession`; invoke from `spa/src/connection.js::landSession`'s
`link.onRestored` before `greetLiveBridge`/`syncRestoredDevice`. Preserve `sessionSwitch` end rules,
`readRequests.js::coordinatedRead` and cache scheduling. Extend bridge carrier/dispatcher/RTC tests
and `spa/test/{sessionRpc,session,sessionSwitch,connectionOffline,cacheSync,readRequests,
transientRead,bridgeApi,cacheOutageSurface}.test.js`:

| Saturation test | Required result |
| --- | --- |
| Hold all eight connections using other sessions; set fixture N=B, fill this opening's backlog, request extra SQL alongside snapshot/control traffic | Extra SQL waits unsent for credit; snapshot/control reach intake and answer before SQL releases. Default N=8, B=16 has additional headroom. |
| Bypass credits and send excess SQL after full pool/backlog | Queue remains ≤B/opening plus eight global running jobs and one parked intake/carrier; no spawned excess waiters or `busy`. Admitted fast traffic/unaffected carriers progress; later unread same-carrier frames may wait. |
| SQL queued/running on A; B attaches; A closes | Opening and B stay usable; A credits return once with transient errors and fresh SQL on B succeeds. A's queued callers/slots/empty markers disappear, B's shared-fold callers survive, and running A replies are discarded. Extend the existing reattachment test. |
| `onRestored` with eight outstanding SQL, both with/without an actual restart | Ledger clears before greeting/pulls and a new SQL read sends/answers. Old/duplicate replies cannot release new credits; stale replaced-session restore callbacks do nothing. Pin B≥2N overlap and bounded repeated-reset fallback. |
| Burst across carriers/priorities; cancel, timeout, late reply; close with parked intake; reopen | Current ledger never exceeds N; each release event wins once, receipts/bare switches do not release. Unsent cancellation cannot send later; close removes queued work/waits. Old completions cannot affect new openings. No path probe from credit waits; transport errors preserve cached paint. |

### Step 3 — Safe snapshots and push facts

Add proposed `app/read_view.rs::{ReadView,publish_delta}`. **In this same step**, fix
`transactions.rs::{persist_plan_record,persist_run_record,
finish_plan_mutation,finish_run_mutation,answer_plan_mutation,answer_run_mutation}` and affected
callers to separate committed canonical fields as in §2. Change
`board/views.rs::{board_list,work_items}`, `board/cache.rs` publication,
`projects/list.rs::{defer_project_list,project_list_rows}`, `projects/parked.rs`,
`workspaces/mod.rs::{workspace_list,workspace_get,adopt_legacy_workspaces}`,
`runtime/agents/endpoints.rs::{agent_list,entity_agent_root,agent_digest}`,
`board/attention.rs::agent_unread`, `config/compaction.rs::compaction_workspace_of`,
`config/settings.rs::settings_get`, `session_summaries.rs` producers and
`app/facts.rs::{entity_facts,locked_facts,bus_with_sources}` / `changes.rs`. Include
`runtime/review_sync.rs::spawn_review_sync`'s post-commit callback and pump/idle completion
publication here; their scope migration remains step 8. Publish identity/canonical-root/unread
changes from lifecycle/pump completions; route board/project/workspace/agent/catalog reads only
once all producers comply. Tests: separate plan and run failed-save cases, then unrelated
successful mutation/refresh: SQL, snapshots and pushes retain last committed fields, no
failed-candidate event. Cover `agent_remove` cleanup/live availability and best-effort
`note_user_activity` separately. Snapshot reads/facts complete with app/writer held, call no
SQL/FS canonicalization, merge concurrent deltas, reject stale refresh/deletion, preserve
first-load rows and #672 lookup counts. Test subscribe/write/read/push and read admission
saturation without refusals.

### Step 4 — Remaining observation reads

Use independent I/O execution in `runtime/deferred.rs::{defer_read,take_deferred,
apply_deferred_stage}` and read handlers: `git/mod.rs::{git_log,git_show,
git_status,git_diff,git_refs,git_unpushed,changeset_diff,worktree_diff}`,
`fs.rs::{fs_list,fs_tree,fs_read}`, `tasks/documents.rs::{plan_doc,plan_stage_doc}`,
`tracker/reviews.rs::{review_get,review_diff}`, `tracker/attachments.rs::tasks_attachment`,
`conversations/attachments.rs::thread_attachment`, `github.rs::defer_github_repos`,
`board/archive.rs::archived_list` and workspace Git-init option readers. Resolve identities from
immutable context; extract resolver adoption/maintenance. Tests: gated filesystem/Git jobs do not
occupy snapshot/SQL execution; path fences, bounded reads, review versions, existing
deadlines/errors and RYOW hold.

The complete read path must bypass `rpc.rs::dispatch_frame`'s app-held update
admission, both `routed` lock acquisitions and `DeliveryRunner::drain`.
Moving only the external job leaves these waits intact. Include `run.diff`
and `task.stage_diff` aliases. Independent I/O is isolation, not evidence
that cold Git/remote work completes in 100 ms; §7 closes that gap.

### Step 5 — Write hold time first, then universal writer gating

**5a ships #709's measured hold-time reduction immediately.** Remove repeated
whole-roster/history/projection work and filesystem probes from finishers,
using owner-local staged deltas and short publication. Capture before/after
`mcp.control`, internal `agent.deliver`, `thread.post`, attention/presence and
tracker-write counts, queue/app wait/hold and full reply duration. Do not
claim the later ack protocol as the fix for a still multi-second lock hold.
**The #709/5a handoff must pass §8's maximum lock/admission wait ≤5 s gate
on both RPC and MCP writes.** This is a release condition, not just a
before/after comparison; a timeout, refusal in the supported workload or a
renamed queue does not pass it.

Remove `reap_orphaned_terminals` from `finish_plan_mutation`/`finish_run_mutation`. In
`runtime/terminals.rs::{reap_orphaned_terminals,spawn_terminal_reaper}` and
`runtime/agents/records.rs::record_agent_session_end`, split a discovery pass from one-owner
cleanup commands, checking owner/tab generation before effect. Move scan/probes outside state
guards; never mutate B's session/board while finishing A. Notification/attention work belonging to
other owners similarly becomes declared follow-up work. Keep required A-local publication before
A's reply; unrelated cleanup is asynchronous. **5b** then adds proposed
`app/scopes.rs::{reserve_all,legacy_exclusive}`. Gate RPC, MCP, pumps/delivery/reclaim/source-sync
and **PR sync/idle** before mutation; the concrete autonomous inventory is below. Their full
legacy operation (not just AppState callback) holds the compatibility reservation until
publication. Tests: A finishes while B's checkout vanishes and B changes concurrently; B cleanup
waits/rechecks and cannot retire its replacement. Legacy background Store mutation conflicts with
scoped work; migrated reads remain independent.

Wire: **no bump or capability**, unchanged contract. Rollback a producer
extraction only with committed-snapshot/failed-save tests still passing;
never put migrated read execution back behind the app mutex. Universal
writer gating remains a prerequisite to narrowing writer families.

### Step 6 — Hot write ack/confirm and SQLite writer boundary

Add proposed `store/writer.rs::StoreWriter` and `app/mutations.rs` owned command completion.
Convert `store.rs::{connection,in_transaction,in_waiting_on_transaction}` and
`store/{tracker,conversations,operations,waiting_on,delivery_intents,user_session,push_keys}.rs`
incrementally through the same writer connection, never a second writer.
**6a** adds the protocol's `store/mutations.rs`, schema migration, durable
admission/owner ordering, status/outbox recovery and boot epoch. Journal
admission bypasses application compatibility ownership; it records intent
without mutating domain state. Execution still takes the appropriate
reservation. Remove long SQL transactions/rewrite work before claiming fast
journal admission. Preserve native review transaction fences.

**6b** enables the hottest writes first and ships the SPA/MCP consumer
simultaneously. `mcp.control` is a mixed local control-plane label:
classify `BridgeAction`/`DoneReport`, route reads independently, and enable
tracker comment/update, post/report/topic/name and agent-message writes first.
`agent.deliver` is internal; retain stable child delivery IDs for
`MessageAgent`/`MessageWorkspaceAgent` and existing transcript/assignment
receipts. Public `thread.post`, `entity.seen`, `user.present`, `tasks.comment`
and `tasks.update` get durable ack, then canonical confirmation. Enable an
individual method only after its whole producer/recovery closure is tested.

Split owners out of
`app.rs::AppState`; change the transaction finishers,
`tracker/mod.rs::{commit_task_write,publish_task_write,tasks_create,tasks_update,tasks_comment}`,
tracking/waiting-on writes, `conversations/post.rs::thread_post`, conversation choice/watch
handlers, `session_summaries.rs::{note_user_activity,user_present}` and
`push_keys.rs::{register_push_key,revoke_push_key}` plus their MCP entry paths. Stage user
activity through its small service without a long device lock. Tests: sibling workspaces/different
projects reach work barriers together; same-owner writes serialize; project deletion fences only
its descendants; reverse-order multi-scope acquisition cannot deadlock; no app guard waits for
SQL; failure leaves committed projections unchanged; disconnect after commit still publishes;
receipts/preconditions and all affected post-confirmation reads hold.
Additional tests: ack before a held executor, no ack on journal failure,
same-ID retry before/after restart, stable nested message IDs, per-owner MCP
ordering, old-generation refusal, partial failure, lost/reordered confirms,
reload before/after ack and cached paint throughout epoch changes. MCP reads
must not wait for a write permit or app mutex.

Wire: **3.24.0** foundation, `bridge.epoch`, `writes.status` and
`writes.subscribe`. The internal `ack_confirm_methods` allowlist starts empty
in 6a, with MCP actions enabled separately in 6b. Add request metadata,
typed ack/confirmation fixtures and all-push epoch stamping. Per Zech’s
2026-10-21 decision, bridge and SPA ship together and the SPA requires this
version: no `write.mode`, legacy final-reply branch or ack-confirm capability
negotiation. Forward migration, durable identity, status/outbox and intact-store
restart recovery remain required; rollback-build/downgrade tests and migrated-DB
rollback guarantees are dropped. See protocol §7.

### Step 7 — Remaining semantic writes, one patch per family

First finish common task/conversation/config setters from step 6's old
scope (create/assign/watch/read marks/dependencies, roster/settings and
notification keys); then the external families below. Each row in §5's
policy inventory must have durable admission, an accepted cache projection,
an applied-result validator and a crash recovery policy before activation.
Short writes still obey ack/confirm; “already fast” does not mean bypass
the journal. Retain acceptance-only meanings for service jobs such as size
measurement; expose their job IDs and later domain progress separately.

Make deferred outcome/reservation state request-owned in
`runtime/deferred.rs::{dispatch_deferring,defer_job,reserve_row,release_row}`; remove
`git/deferred.rs::ScopedGitCall::run`'s state reentry. Each patch uses scope/work/apply, the
existing operation-specific rollback and §3's order.

| Patch / functions | Tests for that patch |
| --- | --- |
| `git/mod.rs::{apply_git,git_stage,git_commit,git_fetch,git_pull,git_push,git_stash,git_stash_pop,git_discard,git_merge_abort,git_checkout_ref}`; `fs.rs::{fs_mkdir,fs_write,fs_create_directory,fs_create_file}`; `fs/uploads.rs::fs_upload_*` | Same physical repo aliases serialize, independent repos overlap; source movement, RYOW, upload ownership/quotas/abort and session-close races. |
| `workspaces/mod.rs::{workspace_create,workspace_retry,workspace_finish,workspace_rename,workspace_ensure_conversation}`, `workspaces/directories.rs::{workspace_add_directory,workspace_remove_directory}`, `workspaces/deletion.rs::workspace_delete`, `workspaces/lock.rs::workspace_set_locked`, `workspaces/reclaim/explicit.rs::workspace_reclaim`, `workspaces/git_initialization.rs::workspace_init_git`; `runs/lifecycle.rs::{run_message,run_abandon,run_adopt}`, `runs/review.rs::{run_request_changes,run_git_action}`, `worktrees/dispatch.rs::branch_dispatch` | Sibling progress, deletion/reclaim vs delayed Git/terminal creation, failed provisioning/cancellation and existing restart cleanup. |
| `projects/requests.rs::project_*`, `projects/source_update.rs::project_update_source`, `projects/deletion.rs::project_delete`, `projects/conversation.rs::project_ensure_conversation`, `projects/base_sync.rs::project_sync_source`; `config/persistence.rs::{config_value_with_project,persist_config}`, `config/settings.rs::settings_set`, `config/compaction_wire.rs::{set_project_compaction,set_workspace_compaction}` | Config serialization retains concurrent project/workspace edits; disk failure, descendant deletion, parked IDs and stale source generations. |
| `tracker/reviews.rs::review_*` and `tracker/reviews/{opening,publication,merge,lifecycle}.rs`; `captures/mod.rs::{capture_create,capture_answer,capture_cancel,capture_reroute}` | Explicit/background review conflicts and deletion, receiver-lock order, existing capture recovery, cross-owner destinations and version guards. |

Wire: proposed **3.25.0**, extend the internal `ack_confirm_methods`
and MCP action allowlists as each family is qualified.
No generic “all writes supported” flag before registry coverage passes.
Tests add process-death interleavings around each external claim/effect,
staged attachment retention, upload offset/hash deduplication, partial
effects, multi-owner deletion and terminal-outbox persistence failure.
The internal allowlist controls new method enablement. Binary rollback onto
the migrated journal and rollback-build tests are out of scope under Zech’s
2026-10-21 decision; status/idempotency and intact-store restart recovery
remain required for every enabled family (protocol §7).

### Step 8 — Session/autonomous completion and bounded read coverage

Migrate each following producer from the step-5 compatibility gate to explicit scopes; no producer
may bypass the gate while only its callback is coordinated.

| Producer / changed functions | Ownership, completion and tests |
| --- | --- |
| `runtime/review_sync.rs::spawn_review_sync`; **root** `reviews/sync/worker.rs::Worker::turn_at`, `reviews/sync/reconcile.rs::{reconcile_background,reconcile_with_wait,prepare,publish_locked,persist_observations}`, `reviews/sync/observations.rs::persist`, `reviews/sync/recovery.rs::CaptureJournal::{acquire_with_wait,recover,begin,finish,finish_registry}`; `reviews/publication/pins.rs::with_received_snapshot_locked`, `reviews/publication/explicit/push.rs`; `store/reviews/pull_requests/sync.rs` candidate/snapshot/observation writers | Reserve task project + review workspace + affected destination owners before journal/Store mutation; observe/revalidate binding generations. Preserve §3's lease/ref/version order; no legacy direct Store bypass. After SQL/native leases release, publish task/review/workspace projections, then changes. Race reconcile against explicit review update/merge/delete; assert stale work cannot resurrect, misattribute authorship or emit pre-publication changes. |
| `runtime/terminals.rs::{spawn_idle_monitor,sweep_idle,mark_idle_tasks}`; `captures/routing.rs::{reap_finished_router_sessions,settle_router_session}`; `runtime/pumps.rs::capture_conversation_names`, `runtime/agents/endpoints.rs::note_self_report` | Discover independently, then reserve each canonical plan/run owner, and device capture/router owner separately. Recheck exact agent instance, activity/delivery generation and capture authority before demotion/retirement; preserve usage-limit/undelivered-turn exclusions. Test stale idle/name observations against new delivery/replacement, another-owner progress and router reassignment. |
| `runtime/pumps.rs`, `runtime/delivery/runner.rs::DeliveryRunner`, `tracker/{waiting_on,delivery_service}.rs`; workspace reclaim/sizes and project source-sync services | Capture exact conversation/workspace/project owners and retain generation guards; replace whole-app weak handles. Tests cover old EOF/receipt vs replacement, completion authority/reassignment, reclaim/source-sync versus owner deletion and unrelated-owner progress. |
| `runtime/terminals.rs::session_hello/term_*`, `runtime/agents/endpoints.rs::{agent_attach,agent_start,agent_interrupt}`, `app/mcp.rs::{Turn,answer_off_the_workers,apply_off_the_socket}`, session registry/reaper | Session owner plus canonical target owner; preserve terminal ordering and opening fences, move process spawning off state guards. Test close/reopen, MCP read isolation, no late attachment and one-worker runtime progress. |
| `app/{updates,watchers,streams}.rs`, `api/v1/changes.rs::{with_session,changes_subscribe,changes_unsubscribe}`, `runtime/off_the_workers.rs::off_the_workers` | Explicit device/session/service ownership and source sets; tests cover subscriber teardown, service callbacks, source movement and liveness/push runtimes under held state locks. |

Autonomous producers need scope/publication discipline but no invented
client ack. Work induced by a journaled command retains its identity.
Complete the MCP read/action/report manifest and session-bound policies from
protocol §§3/6. Apply the settled real-time/session exceptions in §5: those
calls retain ordered final replies and epoch fences and are not journaled.
Finish §7's eight
omitted legacy/snapshot reads and negotiated bounded, cached observation
readers. Require the current SPA and MCP to use those forms against a capable
bridge, including the `cacheSync.js` task-list path described in §7; modern
traffic may not be classified away as legacy to pass the gate. Pull
independent read fixes into earlier releases; do not wait
for every external write to migrate.

Wire: proposed **3.26.0**, extend write/MCP allowlists, add `reads.bounded`
with an exact per-method support list for new paged/cached observation
contracts. Epochs are already in 3.24.0; no new mechanism per carrier.
Tests cover terminal order/load, ended openings, stale process handles,
all MCP tools/reports, cold/large read values and old/new paging behavior.
Rollback new bounded readers via negotiation while retaining legacy shapes;
retain operation status/recovery for all accepted writes. Legacy unbounded
paths are not certified ≤100 ms merely because the new mode is disabled.

### Step 9 — Remove compatibility and accept

Remove remaining request/fact/service access to `Mutex<AppState>` and legacy Store guards from
`app.rs`, `app/rpc.rs`, `api/v1/mod.rs`, `store.rs` and lane dispatch. Retire single deferred
slots only when no callers remain. Update `ARCHITECTURE.md` and older concurrency specs to the
shipped boundary. Tests: exact verb coverage, readonly contexts cannot acquire write services;
repeat step-1 before/after samples plus bounded pool-wait/query checks; held writer/app/Git work
cannot prevent snapshot/SQL reads; no capacity-refused reads; scoped parallelism/order,
reset/delete/failure races and every affected post-confirmation view.
**The quantitative ≤100 ms gate in §7 and maximum lock/admission wait ≤5 s
gate in §8 are required**, alongside hot durable-ack p99 ≤100 ms under the
same mixed load. The maximum gate covers RPC and MCP and may not be replaced
by a percentile, timeout or renamed queue. Show acceptance latency separately
from execution/confirmation latency. Audit all 157 baseline RPCs, new verbs
and every MCP action/report for one policy; no hidden read-side mutation.
Check WAL/memory growth under paged
load and preserve wire/cache rendering contracts. Run audited tests, ratchet, Clippy/fmt, Semgrep
and Gitleaks.

Wire: **no bump for internal removal**. Preserve legacy 3.x full-result
shapes while moving the current SPA to bounded reads; retiring an old shape
would require a separate major version. Rollback keeps compatible readers,
migrated-schema support and status/idempotency for retained commands even
after draining. Restore a tested internal execution path only, never
synchronous legacy replay of accepted IDs.

## 5. Verb inventory and known AppState effects

Registered inventory: **138 v1 names**, plus **16 legacy and 3 QA names**. M = in-memory read, S =
SQL read, W = write (including hidden maintenance), I/O = filesystem/Git/CLI observation. Mixed
reads are named explicitly. Scopes are proposed ownership, not current locks. The table merges
handler locations and known direct/helper fields; helpers with no direct field access inherit
their effects from the named implementation, not an extra empty row. The
original table's “now” describes the pre-step-1 baseline, not today's shipped
SQL/snapshot routing; use `api/effects.rs` and `ARCHITECTURE.md` for current
behavior. The policy table below uses the amendment baseline. A group is a conservative
field set, not proof every member is written by that verb. Unresolved transitive closures stay
behind the compatibility gate.

- **P**: `projects`; **W**: `workspaces`, `workspace_lifecycle`, reclaim guards.
- **E**: `plans`, `runs`, nested agents/conversations; `session_summaries`, `session_seen`, `session_registry` where used.
- **B**: `board`, `vanished_run_sweep_in_flight`, `pending_rows`, cache scheduling/`usage_limits` inputs.
- **C**: `projects_dir`, `default_harness`, `project_agent`, `role_models`, `agent_modes`, `isolation`, `watch_agent_filed_tasks`, `compact_above_tokens`, `compaction_policies`, `config_path`, `reclaim_settings`, `reclaim_policy` (only the setting's subset).
- **T**: `store`; task joins additionally P/W/E and `user_session`.
- **R**: `session_registry`, `delivery_queue`, `operation_ledger`, `router_sessions`, `dispatched_task`, `task_completion_authority`, `spent_task_completion_operations`, `pending_task_completion_authority`, `task_delivery_in_flight`, `reminded_holdings`, `agent_failures`, `compactions`, `resetting_conversations`, `streams`; per-writer closure remains an audit gate.
- **G**: P/W/E scope inputs, board checkout/cache state, `deferred_work`, `self_handle`, `reclaim_reserved`.
- Common applicable helpers: `deferred_work`/`deferred_result_check`/`active_deferred_filesystem_jobs`, `pending_rows`, `changes`, `watchers`; persistence also B/T/session summaries, notification also `notifier`/`notify_throttle`; runtime also `agent`, `harness`, `session_locator_factory`, `resume_id_probe`, `live_roster`, `usage_limits`, `self_handle`, `push_runtime` and R. RPC activity uses `user_session`/Store per `USER_ACTIVITY_VERBS` and `INTERACTION_VERBS`, not every write. Capture paths/dependencies at construction.

### v1 methods

| Verbs | Current → target read form | Write scope | Known AppState fields | Handler targets (`bridge/src/`) and effects |
| --- | --- | --- | --- | --- |
| `board.list` | W now; target M published board | device maintenance, then no read lock | B, E, P, W, T | `app/board/views.rs::board_list`. Attention persistence, scratch-directory recuts, cache/vanished-run sweeps and watcher refresh leave reads. |
| `archived.list` | snapshot + filesystem read | none | B, E, P, W | `app/board/archive.rs::archived_list`. WorkspaceRegistry::finished_records reads legacy registry files; move observation to background for M. |
| `project.list` | W now; M published project rows after maintenance/probe extraction | device registry maintenance, then none | P, E, C, deferred_work | `app/projects/list.rs::defer_project_list`. Revives parked projects and rebinds retained entities before deferred volume/repository probes. |
| `project.create` | write | device registry creation + new project | P, C, deferred_work | `app/projects/requests.rs::project_create`. Allocates/clones source directories and persists config; reserve new identity before work. |
| `project.delete` | write | device registry + project + all affected workspaces | P, W, E, R, C, T, isolation, default_harness, projects_dir, session_registry, delivery_queue, active_deferred_filesystem_jobs, project_deletion_in_progress | `app/projects/deletion.rs::project_delete`. Global deletion/filesystem guards today; retirement and retained-owner cleanup span descendants. |
| `project.init_git`, `project.set_remote`, `project.add_source`, `project.remove_source`, `project.set_isolation` | write | project; registry/config publication is device metadata | P, C, deferred_work | `app/projects/requests.rs::{project_init_git,project_set_remote,project_add_source,project_remove_source,project_set_isolation}`. Source/config mutations; shared config serialization is separate from owner scope. |
| `project.update_source` | write | project + every workspace checkout affected by changed source | P, W, source_moves, C, deferred_work | `app/projects/source_update.rs::project_update_source`. source_moves and remote origins can affect multiple workspace checkouts. |
| `project.sync_source` | write (service request) | project | P, source_sync_requested, source_sync_nudge | `app/projects/base_sync.rs::project_sync_source`. Queues source_sync_requested and nudges service; later completion publishes observations. |
| `project.ensure_conversation` | write | project | P, E, R, store, state_root | `app/projects/conversation.rs::project_ensure_conversation`. May create project scratch/run/pin; already-present case stays idempotent. |
| `project.set_compaction`, `workspace.set_compaction` | write | project / workspace, plus device config serialization | C, P, W, changes | `app/config/compaction_wire.rs::{set_project_compaction,set_workspace_compaction}`. Shared config persistence; current workspace response also invokes hidden workspace maintenance. |
| `capture.get` | snapshot read | none | captures | `app/captures/mod.rs::capture_get`. Map lookup and row rendering only. |
| `capture.create`, `capture.answer`, `capture.cancel`, `capture.reroute` | write | device capture/router; reroute adds selected project/workspace | captures, router_sessions, store; E/R/P downstream | `app/captures/mod.rs::{capture_create,capture_answer,capture_cancel,capture_reroute}`. Persist capture and start/abandon router; reroute resolves target later as a separate scoped command. |
| `settings.get` | snapshot + external probe now; target M cached observation | none | C, P | `app/config/settings.rs::settings_get`. account_availability probes first project volume; publish observation first. |
| `settings.set` | write | device config | C, P, changes, reclaim_nudge | `app/config/settings.rs::settings_set`. Prospective config validation/persistence precede memory; preserve disk-failure behavior. |
| `models.list` | snapshot read | none | default_harness, agent_modes, role_models, cli_readings | `app/config/settings.rs::models_list`. Cached CLI readings and model catalog; no CLI invocation here. |
| `user.present` | write | device user session | user_session, store, changes | `app/session_summaries.rs::user_present`. Persists user-session activity and invalidates task/session views. |
| `changes.subscribe`, `changes.unsubscribe` | write; exempt (real-time) | session/device subscription registry; watcher paths may span projects | changes, watchers, deferred_work | `app/projects/source_scope.rs::check_source_subscription`; `app/runtime/deferred.rs::defer_watch`. Source validation, ChangeBus subscriptions and deferred watcher reconciliation; carry SessionSender explicitly. |
| `git.log`, `git.show`, `git.status`, `git.diff`, `git.changeset_diff`, `git.refs`, `git.unpushed`, `worktree.diff`, `run.diff`, `task.stage_diff` | snapshot + off-lock Git read; scope can write today | none after resolver maintenance extraction | G | `app/git/mod.rs::{git_log,git_show,git_status,git_diff,changeset_diff,git_refs,git_unpushed,worktree_diff}`; `app/runs/review.rs::run_diff`; `app/tasks/views.rs::task_stage_diff`. Resolvers can adopt legacy workspaces; external observations are not SQL-only. |
| `git.stage`, `git.discard`, `git.commit`, `git.fetch`, `git.pull`, `git.push`, `git.stash`, `git.stash_pop`, `git.merge_abort`, `git.checkout_ref` | write | resolved workspace OR project source; common repository/path lock when aliases share Git | G, B, W, changes | `app/git/mod.rs::{git_stage,git_discard,git_commit,git_fetch,git_pull,git_push,git_stash,git_stash_pop,git_merge_abort,git_checkout_ref}`. defer_git then apply_git may reopen workspace and invalidate caches/attention; preserve source/ref fences. |
| `fs.list` | external filesystem read | none | no direct AppState field | `app/fs.rs::fs_list`. Canonicalizes/browses host directories; no mutable AppState dependency. |
| `fs.mkdir` | write | device/host directory namespace | reclaim_reserved through refusal helper | `app/fs.rs::fs_mkdir`. Host directory namespace is an explicit ownership exception to project/workspace scopes. |
| `fs.tree`, `fs.read` | snapshot + filesystem read; scope may write today | none after resolver maintenance extraction | G | `app/fs.rs::{fs_tree,fs_read}`. FileScope resolution can adopt legacy workspaces; extract it before off-lock I/O. |
| `fs.write`, `fs.createDirectory`, `fs.createFile` | write | resolved workspace/project source or legacy owner | G, W, B, changes | `app/fs.rs::{fs_write,fs_create_directory,fs_create_file}`. Fenced file changes, reopen and invalidate_file_scope; recheck captured source generation. |
| `fs.uploadBegin`, `fs.uploadChunk`, `fs.uploadFinish`, `fs.uploadAbort` | write (session upload lifecycle) | upload owner session + resolved workspace/project/path | uploads, G; W/B/changes at finish | `app/fs/uploads.rs::{fs_upload_begin,fs_upload_chunk,fs_upload_finish,fs_upload_abort}`. Upload ID retains admitted owner/scope; do not reserve a workspace across the whole upload lifetime. |
| `github.repos` | snapshot + external CLI/network read | none | github, deferred_work | `app/github.rs::defer_github_repos`. Deferred external gh request with existing deadline. |
| `harnesses.list`, `bridge.update_status` | snapshot service read | none | harness_inventory / updates | `app/harness_inventory.rs::harness_inventory`; `api/v1/updates.rs::update_status`. Clone service handle outside app lock. |
| `harnesses.refresh`, `bridge.check_update` | write (service request) | device/service | harness_inventory / updates | `app/harness_inventory.rs::harness_inventory`; `api/v1/updates.rs::check_update`. Schedules service observation; not pure reads. |
| `bridge.install_update` | write (service request) | device admission/update | updates, R | `app/updates.rs::update_has_working_agents`. Admission depends on a coherent working-agent/delivery/session view. |
| `task.get` | snapshot + SQL history read | none | plans, E, T | `app/tasks/views.rs::plan_get`. Retired singular task flow; detail_thread_value may read stored history. |
| `task.stages` | snapshot read | none | plans, runs | `app/tasks/views.rs::task_stages`. Retained stage/comment and implementation-progress projection. |
| `task.doc`, `task.stage_doc` | snapshot + filesystem document read | none | plans, store | `app/tasks/documents.rs::{plan_doc,plan_stage_doc}`. Store::read_plan_doc reads files, not SQLite. |
| `entity.seen`, `entity.mute`, `entity.dismiss` | write | canonical conversation/project/workspace owner; branch dismissal project | B, E, store, user_session, changes | `app/board/attention.rs::{entity_seen,entity_mute,entity_dismiss}`. Persist attention marks/mutes/dismissals; branch-only dismissal is project-scoped. |
| `run.request_changes`, `run.git_action`, `run.message`, `run.abandon`, `run.adopt`, `branch.dispatch`, `branch.finish` | write | resolved project/workspace; dispatch/adopt/finish may cross both | E, P, W, B, R, store, deferred_work, qa_agent, default_harness | `app/runs/review.rs::{run_request_changes,run_git_action}`; `app/runs/lifecycle.rs::{run_message,run_abandon,run_adopt}`; `app/worktrees/dispatch.rs::branch_dispatch`; `app/workspaces/mod.rs::workspace_finish_legacy`. Lifecycle/deferred work and delivery cross owners; full helper closure must be audited. |
| `push.registerKey`, `push.revokeKey` | write | device notification key namespace | store | `app/push_keys.rs::{register_push_key,revoke_push_key}`. Validate then Store upsert/delete; transactional-only writer candidate. |
| `tasks.review.get` | snapshot + SQL + filesystem/Git projection read | none | store, projects, workspaces, reclaim_reserved, deferred_work | `app/tracker/reviews.rs::review_get`. Review action/destination projection includes source/Git/reservation observations. |
| `tasks.review.diff` | snapshot + SQL + Git read | none | store, projects, deferred_work | `app/tracker/reviews.rs::review_diff`. Keep immutable review revision bound to deferred Git read. |
| `tasks.review.open`, `tasks.review.push`, `tasks.review.update`, `tasks.review.merge`, `tasks.review.close`, `tasks.review.reopen`, `tasks.review.refresh`, `tasks.review.snapshot`, `tasks.review.act`, `tasks.review.complete` | write | project task + review workspace + any destination/source checkouts | store, P, W, E, R, changes, deferred_work | `app/tracker/reviews/opening.rs::review_open`; `app/tracker/reviews/publication.rs::{review_push,review_update}`; `app/tracker/reviews/merge.rs::review_merge`; `app/tracker/reviews/lifecycle.rs::{review_close,review_reopen,review_refresh}`; `app/tracker/reviews.rs::{review_snapshot,review_act,review_complete}`. Refresh/snapshot persist PR reconciliation; merge/act can affect source destinations. |
| `tasks.list`, `tasks.get` | W backfill now; target S + M identity overlay | project migration; then none for reads | T, E, P, W, user_session | `app/tracker/mod.rs::{tasks_list,tasks_get}`. Move identity/done-at backfill and list-key INSERT out; retain pure legacy projection and live overlays. |
| `tasks.waiting_on.blocks` | snapshot scope + SQL read | none | store, projects | `app/tracker/waiting_on_api.rs::tasks_waiting_on_blocks`. Immutable project context + one SQL transaction removes tracker_task app lookup. |
| `tasks.columns` | snapshot read | none | projects | `app/tracker/mod.rs::tasks_columns`. Validates project then fixed columns_json; no SQL. |
| `tasks.create`, `tasks.assign` | write | project task + target conversation/workspace; may allocate workspace | T, P, W, E, R, deferred_work | `app/tracker/mod.rs::tasks_create`; `app/tracker/dispatch.rs::tasks_assign`. Assignment may provision a checkout and enqueue durable delivery. |
| `tasks.update`, `tasks.comment`, `tasks.waiting_on.add`, `tasks.waiting_on.remove`, `tasks.close`, `tasks.reopen`, `tasks.watch`, `tasks.unwatch`, `tasks.read_through` | write | task project; delivery/linked-owner followups may additionally require workspace | T, E, P, W, R, changes | `app/tracker/mod.rs::{tasks_update,tasks_comment,tasks_close,tasks_reopen}`; `app/tracker/waiting_on_api.rs::tasks_waiting_on_edit`; `app/tracker/tracking.rs::{tasks_watch,tasks_unwatch,tasks_read_through}`. Task/timeline/dependency/identity/delivery effects; include linked-owner follow-ups in the scope set. |
| `tasks.attach` | write | task project + shared attachment store namespace | store, projects, state_root | `app/tracker/attachments.rs::tasks_attach`. Validates task/project then writes fenced attachment bytes. |
| `tasks.attachment` | snapshot scope + SQL + filesystem read | none | store, projects, state_root | `app/tracker/attachments.rs::tasks_attachment`. Task/attachment identity in SQL; bounded bytes read from shared attachment files. |
| `thread.page`, `thread.activity` | M + optional S now; target S + immutable owner admission | none | E, store | `app/conversations/read.rs::{thread_page,thread_activity}`. Target S with owner admission context; SQL metadata and rows share one generation/transaction. |
| `thread.revision` | snapshot read | none | E | `app/conversations/read.rs::thread_revision`. Retained revision snapshots; preserve missing/discarded revision errors. |
| `thread.operation` | snapshot binding + SQL read (memory fallback) | none | E, store, operation_ledger | `app/conversations/post.rs::thread_operation`. Preserve receipt owner/agent validation and Store-free operation_ledger fallback. |
| `thread.attachment` | snapshot binding + filesystem read | none | E, state_root and attachment-root resolver fields | `app/conversations/attachments.rs::thread_attachment`. Binding plus fenced bounded file read; resolver closure remains audit work. |
| `thread.post`, `thread.attach` | write | canonical conversation owner project/workspace; durable delivery may touch task project | E, R, store; attachment paths | `app/conversations/post.rs::thread_post`; `app/conversations/attachments.rs::thread_attach`. Existing operation acceptance/delivery and attachment ownership must survive off-lock execution. |
| `agent.list` | M + filesystem + SQL now; target M after precomputation | none | E, P, W, session_registry, C, usage_limits | `app/runtime/agents/endpoints.rs::agent_list`. entity_agent_root/compaction workspace lookup canonicalize paths; unread can query SQL. Precompute all three before M. |
| `agent.add`, `agent.choose`, `agent.remove`, `conversation.watch`, `conversation.unwatch`, `conversation.reset`, `conversation.settings` | write | canonical project/workspace conversation; removal can preserve task identities across project | E, R, B, P, W, store, C, changes, cli_readings, compact_above_tokens, state_root | `app/runtime/agents/endpoints.rs::{agent_add,agent_choose,agent_remove}`; `app/runtime/agents/records.rs::{set_conversation_watched,set_conversation_context_limit}`; `app/conversations/reset.rs::conversation_reset`. Removal preserves historical task identities; aliases/reset/roster effects cross canonical owners. |
| `workspace.list`, `workspace.get` | W now; target M published workspace views | global workspace maintenance currently; target none | W, P, E, C, T | `app/workspaces/mod.rs::{workspace_list,workspace_get}`. Adoption, capabilities/manifests, finished-state Git checks and stored review summaries leave reads. |
| `workspace.git_init_options` | snapshot + external probe read; scope may write today | none after legacy adoption extraction | projects, workspaces | `app/workspaces/git_initialization.rs::workspace_git_init_options`. Probes workspace/original source; resolver can adopt legacy records today. |
| `workspace.create`, `workspace.retry` | write | project allocation + target workspace | W, P, C, deferred_work | `app/workspaces/mod.rs::{workspace_create,workspace_retry}`. Provisioning and source/base-sync use reserve/work/apply; preserve captured sources. |
| `workspace.ensure_conversation` | write | workspace + project binding | workspaces, projects, runs, default_harness, store | `app/workspaces/mod.rs::workspace_ensure_conversation`. Creates run/binding; finish_run_mutation and caller rollback currently participate. |
| `workspace.init_git` | write | workspace and/or project source depending target; both = multi-scope | W, P, deferred_work | `app/workspaces/git_initialization.rs::workspace_init_git`. Target may be workspace, original source, or both; reserve corresponding union. |
| `workspace.add_directory`, `workspace.remove_directory`, `workspace.rename`, `workspace.set_locked` | write | workspace; add source also project/source coordination | W, P, B, deferred_work | `app/workspaces/directories.rs::{workspace_add_directory,workspace_remove_directory}`; `app/workspaces/mod.rs::workspace_rename`; `app/workspaces/lock.rs::workspace_set_locked`. Rename/lock use global deferred-job guards today; response detail also runs maintenance. |
| `workspace.finish`, `workspace.delete`, `workspace.reclaim` | write | workspace + project-linked task/conversation cleanup + source/path cleanup | W, P, E, R, T, B, reclaim_reserved, deferred_work | `app/workspaces/mod.rs::workspace_finish`; `app/workspaces/deletion.rs::workspace_delete`; `app/workspaces/reclaim/explicit.rs::workspace_reclaim`. Linked task identities, retirement and source/filesystem cleanup require declared multi-owner scope. |
| `workspace.measure_sizes` | write (queue request) | selected workspace set or device-wide queue admission | workspaces, workspace_lifecycle, size_requests | `app/workspaces/sizes.rs::workspace_measure_sizes`. Enqueues SizeRequests; later walker updates workspace_lifecycle. |

### Session-scoped, legacy and QA arms

| Verb/arm | Classification | Write scope | Current AppState fields / service effects; implementation evidence |
| --- | --- | --- | --- |
| `ping` | M/static | none | `app/rpc.rs::pong`; intake fast path uses no AppState. Direct legacy dispatch still passes routing bookkeeping. |
| `bridge.stats` | M/metrics | none | `session_scoped` reads `FrameTimer::clock().stats()`, not AppState; current queue dependence is in §1. |
| `rtc.offer`, `rtc.ice`, `rtc.close` | W; exempt (real-time) | device peer service, per session | `app/rtc.rs::{rtc_offer,rtc_ice,rtc_close}` uses the captured `peers` slot and mutable SessionPeers; no app mutex for signaling. |
| `session.hello` | W; exempt (real-time) | device session subscriptions | `runtime/terminals.rs::session_hello`: `changes`, `qa_agent`, `updates`, `cli_readings`, `harness_inventory`, `update_subscriptions`, `models_subscriptions`, `harnesses_subscriptions`; FrameClock client registry records the declaration separately. Idempotent subscription is still a write. |
| `term.list` | M + scope I/O/possible maintenance now; target M | none after extraction | `runtime/terminals.rs::{term_list,shell_tabs_for}` reads `session_registry`; `terminals/scope.rs::resolve_terminal_scope` traverses `projects`, `runs`, `workspaces` and external checkout/cache resolution. The same legacy-adoption/cache caveat applies as for file scope reads. |
| `term.create` | W | project source or workspace; device terminal-cap reservation | `runtime/terminals.rs::term_create`: `project_deletion_in_progress`, scope fields above, `source_moves`/source guards, `reclaim_reserved`, `session_registry`, `term_shell`, `changes`; RPC success also touches `board`, `user_session`, `store`. It currently spawns shell/PTY under the app guard. Move spawning to reserve/work/apply without losing the cap or deletion recheck. |
| `term.attach` | W; exempt (real-time) | session + resolved project/workspace terminal | `runtime/terminals.rs::term_attach`: `session_registry`, source/owner scope validation; cloned terminal handle attaches/resizes its screen off app lock. Screen subscription is the mutation even when the registry itself is only read. |
| `term.input`, `term.resize`, `term.ack` | W; exempt (real-time) | addressed terminal/session within project or workspace | Same-named functions in `runtime/terminals.rs` resolve through `session_registry` and owner indices, then write PTY/resize/screen cursor off lock. These calls are not journaled and retain ordered final replies and epoch fences. Preserve detached-session behavior and never replay uncertain input. |
| `term.close` | W | addressed project/workspace terminal | `runtime/terminals.rs::term_close` resolves `session_registry`/scope then `retire_tab`; registry, screen close/change effects and retirement. Agent tabs are refused. Exact retirement helper transitive field closure is an implementation audit gate. |
| `agent.attach` | W; exempt (real-time) | canonical project/workspace agent + client session | `runtime/agents/endpoints.rs::agent_attach`: resolves `plans`, `runs`, project/workspace binding and thread guard; `session_registry` may allocate a waiting screen, then attaches screen clients. Never classify this as a read because its result is a snapshot. |
| `agent.start` | W | canonical project/workspace conversation | `runtime/agents/endpoints.rs::agent_start`: `plans`, addressed `runs`/owners/choices; `delivery_queue`, `board`, then `DeliveryRunner::drain`; RPC adds `user_session`/`store`/`changes`. Delivery's remaining fields are in R and require full closure audit before narrow admission. |
| `agent.interrupt` | W | canonical project/workspace conversation | `runtime/agents/endpoints.rs::agent_interrupt`: reads `plans`, binding/generation, `session_registry`; invokes the captured exact session's interrupt off lock. RPC success updates `user_session`/`store`/`changes`. Keep instance check across replacement. |
| `stream.start` | W, QA-only; exempt (real-time) | device QA stream, then per stream | `app/streams.rs::stream_start` increments `next_stream`, inserts `streams`, background `append_event` mutates that stream. Reads `qa_agent` at dispatch. |
| `stream.events`, `stream.state` | M, QA-only | none | Same-named `app/streams.rs` methods read `streams`, gated by `qa_agent`; `&mut self` signature alone does not make them writers. |
| close frame (not a method) | W; exempt (real-time) | ending session/device service ownership | `app/rpc.rs::dispatch_frame` → `runtime/terminals.rs::drop_session`: `uploads`, `peers`, `session_registry` screen detach; subscription maps for updates/models/harnesses, `changes`, `watchers`, FrameClock client registry. `FrameHandler::on_session_end` separately drops ended uploads. Preserve opening generation and ordering. |

### #710 write acknowledgement policy (exhaustive)

The first column below contains each of `api/effects.rs::WRITE_METHODS`'s
**107 baseline names exactly once**. “Ack early” separates durable intent
from potentially slow application. “Already fast” means a bounded/service
operation can normally apply and publish at admission, then ack and confirm
back-to-back; it is a design classification, **not measured latency evidence**.
Both semantic-write policies use the coordinated wire protocol, durability and
failure policy. **Exempt (real-time)** calls use ordered final replies and
epoch fences, with no durable early ack or journal. Older clients are not
supported by this protocol rollout (Zech’s 2026-10-21 decision). The settled exception is enumerated below, including QA and
subscription controls; a write effect classification alone does not imply
journaling.

| Write verbs | Policy | Acceptance/result and recovery boundary |
| --- | --- | --- |
| `thread.post`, `run.message`, `run.request_changes` | ack early | Save normalized message/IDs and existing delivery receipt; confirm transcript publication, not completion of the agent's turn. |
| `tasks.comment`, `tasks.update`, `entity.seen`, `user.present` | ack early | Hot path first; save bounded intent/accepted projection, then canonical SQL and affected attention/session projections. |
| `tasks.create`, `tasks.assign` | ack early | Reserve task/assignment/child operation IDs; preserve durable delivery authority and generations. |
| `tasks.close`, `tasks.reopen`, `tasks.watch`, `tasks.unwatch`, `tasks.read_through`, `tasks.waiting_on.add`, `tasks.waiting_on.remove` | ack early | Apply transactional tracker change; separate induced delivery/notice work with stable identities. |
| `entity.mute`, `entity.dismiss`, `conversation.watch`, `conversation.unwatch`, `conversation.settings` | ack early | Accepted attention/config overlay; confirm only after save/publication. |
| `agent.add`, `agent.choose`, `agent.remove`, `agent.start`, `agent.interrupt`, `conversation.reset` | ack early | Reserve exact agent/session/generation; retain retirement/reset/interrupt fences and ambiguous-effect policy. |
| `thread.attach`, `tasks.attach` | ack early | Ack only after durable bounded blob staging and descriptor allocation; no success based solely on a temporary source path. |
| `project.create`, `project.delete`, `project.ensure_conversation`, `project.init_git`, `project.set_remote`, `project.add_source`, `project.remove_source`, `project.set_isolation`, `project.update_source` | ack early | Durable target/source set; external work and shared config publish under explicit scopes. |
| `settings.set`, `project.set_compaction`, `workspace.set_compaction` | ack early | Validate and save intent, then atomically replace/reconcile config and publish canonical settings. |
| `workspace.create`, `workspace.retry`, `workspace.ensure_conversation`, `workspace.init_git`, `workspace.add_directory`, `workspace.remove_directory`, `workspace.rename`, `workspace.set_locked`, `workspace.finish`, `workspace.delete`, `workspace.reclaim` | ack early | Stable workspace/source IDs and staged lifecycle; replay only proven unapplied steps. |
| `branch.dispatch`, `branch.finish`, `run.abandon`, `run.adopt`, `run.git_action` | ack early | Persist complete owner/ref set and child IDs; preserve existing cleanup/review contracts. |
| `git.stage`, `git.discard`, `git.commit`, `git.fetch`, `git.pull`, `git.push`, `git.stash`, `git.stash_pop`, `git.merge_abort`, `git.checkout_ref` | ack early | Expected refs/path identities, claim before effect, verb-specific reconciliation of uncertain outcomes. |
| `fs.mkdir`, `fs.write`, `fs.createDirectory`, `fs.createFile`, `fs.uploadBegin`, `fs.uploadChunk`, `fs.uploadFinish`, `fs.uploadAbort` | ack early | Durable blob/offset/hash and exact owner opening; no stale-handle or changed-destination replay. |
| `tasks.review.open`, `tasks.review.push`, `tasks.review.update`, `tasks.review.merge`, `tasks.review.close`, `tasks.review.reopen`, `tasks.review.refresh`, `tasks.review.snapshot`, `tasks.review.act`, `tasks.review.complete` | ack early | Preserve version/journal/native-ref locks and publication fences; reconcile partial external effects. |
| `capture.create`, `capture.answer`, `capture.cancel`, `capture.reroute` | ack early | Save capture/router command and stable downstream identities; later routing is domain work. |
| `term.create`, `term.close` | ack early | Exact terminal instance, durable claim and ordering against the terminal stream; ambiguous lifecycle effects are never automatically repeated. |
| `push.registerKey`, `push.revokeKey` | already fast | Bounded transactional key change and saved outcome; no unrelated finisher work. |
| `project.sync_source`, `workspace.measure_sizes`, `harnesses.refresh`, `bridge.check_update`, `bridge.install_update` | already fast | Confirm durable service-job admission with job ID; later scan/download/install is separate observable domain state. |
| `session.hello`, `changes.subscribe`, `changes.unsubscribe`, `term.attach`, `agent.attach`, `term.input`, `term.resize`, `term.ack`, `stream.start` | exempt (real-time) | No journal or durable early ack. Keep ordered final replies, exact opening/epoch and no automatic replay; settle uncertain in-flight calls on epoch change per protocol §4. |

Other §5 writes have explicit dispositions too:

| Entry | Policy | Boundary |
| --- | --- | --- |
| `rtc.offer`, `rtc.ice`, `rtc.close` | exempt (real-time) | No journal; keep ordered final replies and epoch metadata for the current connection lifetime. |
| close frame | exempt (real-time) | No journal; transport teardown fences its commands and terminates session-scoped unapplied ones. It is not replayed as a write RPC. |
| `mcp.control` write actions and coding/router reports | ack early | Classify nested `BridgeAction`/`DoneReport`, not the outer label. All semantic writes use journaled identity/owner order; reads bypass the write queue. Post/topic/name/report and agent messages ship with hot tracker writes in step 6, remaining actions in steps 7/8. |
| internal `agent.deliver` | ack early | Link accepted message/assignment IDs to existing provider receipt; do not claim provider success at admission or duplicate a claimed delivery. |

New protocol verbs also need effect entries and fixtures: `writes.status`
and paged `writes.result` are SQL reads; `writes.subscribe` is an
**exempt (real-time)** session subscription, with no journal and ordered
final replies/epoch fences. New bounded-observation support must register each new method/action
before capability announcement. A set-equality test against the registry and
exhaustive MCP enum matches prevent omissions as main changes.

## 6. Top risks and remaining unknowns

1. **Hidden ownership:** callbacks and repositories cross owners. Audit each helper before
   narrowing its reservation; retain PR/native fences and the maintenance split.
2. **Committed-state divergence:** failed saves/reset/stale refresh can mix generations. Keep
   committed fields, transactional history and versioned publication; test failure then unrelated
   work.
3. **Contention moves:** pools, writer, config, publication and credits can queue. Workload/SQL
   share and sizing remain unmeasured; compare waits without hiding reads as refusals. PR fences
   may delay writes, never WAL reads.
4. **Durable acceptance is a new contract:** generic retries must not duplicate
   provider, Git, filesystem or PTY effects. Retain command tombstones, stable
   child identities and per-verb recovery; failures may be partial/unknown.
5. **Qualification remains:** protocol §8 records the settled control
   exemptions, bounded-read target and process-restart durability scope.
   This planning commit makes no runtime performance/compliance claim;
   implementation must meet the read and write-wait release gates below.

## 7. ≤100 ms reads: remaining gaps and acceptance gate

Steps 4/5 remove contention sources, not the cost of fresh arbitrary I/O.
The amendment baseline contains no measured post-step-4/5 p99 result.
Independent reads can still exceed 100 ms for these concrete reasons:

| Read surface / evidence | What remains after isolation | Work required |
| --- | --- | --- |
| All remaining `app/rpc.rs::routed` reads; `runtime/delivery/runner.rs::DeliveryRunner::drain` | App-held admission/apply and unrelated delivery work can still delay a nominal read | Captured read services bypass the entire routed write lifecycle; pure read completion never invokes delivery or finishers. |
| `capture.get`, `task.stages`, `tasks.columns`, `thread.revision`, `stream.events`, `stream.state` | Classified snapshot reads but absent from `INTAKE_SNAPSHOT_METHODS` | Publish immutable projections; incremental QA stream checksum, bounded stream pages, no per-read stage/comment rescans. |
| `task.get`, `term.list`; `archived.list` already in step 4 | Optional SQL history, filesystem scope resolution and legacy registry scans | Capture admitted identities; independent SQL for history; published terminal scopes/archive records. No resolver adoption on read. |
| Six SQL reads, especially `tasks.list/get`, plus `writes.status` | Read-pool checkout, transaction/projection cost and `read_pool.rs`'s five-second SQLite busy timeout; unlimited `tasks.list` default | Index/query-plan audit, bounded timeline/summary projection, short transactions and fair read-only workers. Introduce negotiated list/detail paging; keep old full-result semantics until explicitly retired. Bound/rework SQLite contention rather than masking it as `busy` or an empty result. |
| `tasks.review.get/diff`, `tasks.attachment` | Existing `tracker_task`/review loading can use the writer Store before independent I/O | One readonly transaction for identity/revision; immutable source/reservation context; separate bounded blob/Git observations. A cloned Store is still the writer connection. |
| `fs.list/tree/read`, `task.doc/stage_doc`, `thread.attachment` and task attachments | Whole-directory sorting, whole-document reads, legacy 32 MiB media and up-to-5 MiB attachment encoding | Paged directory indexes and body chunks, bounded decoding/encoding, no app/path locks. Benchmark valid maximum pages and malformed inputs; retain no-follow/root/source fences. |
| Git/status/log/show/diff/refs/unpushed/changeset/worktree/run/stage reads; workspace Git-init options | Cold repository work, full diffs and source probes are not constant time | Versioned cached observations, immutable revision-bound body pages, background producers and explicit refresh commands. Keep canonical common-dir/source-generation fences. |
| `github.repos` (`github.rs::GH_DEADLINE`, 20 s) | External CLI/network result cannot promise 100 ms freshness | Cached authorized repository pages and explicit refresh-job state; retain timeout/error evidence for refresh, not on the read path. |
| Nine already-published reads, including full `workspace.get` history | JSON clone, result validation, serialization/encryption can dominate even an atomic root load | Cache typed bounded chunks/encoded stable pieces where safe; negotiated history/list paging. Measure full answer, not root-load time. |
| MCP read actions in `mcp.rs` and `app/mcp.rs` | `ControlPlane::Turn`, authentication and action routing still take write permits/app locks | Capture immutable authenticated principal/thread-generation context and route reads to SQL/snapshot/I/O lanes before write permits; preserve all owner authorization. Ship this with step 6. |

### Bounded/cached observation contract

Under `reads.bounded`, a per-method support list negotiates bounded pages:
initial limits of 100 list/directory entries and 256 KiB decoded body chunks,
plus a 512 KiB total encoded application-result ceiling (tune downward if
maximum-valid-page measurements miss budget). Large embedded strings,
task timelines/details, review results and batched `writes.status` outcomes
need cursors or revision-bound result references, not just item-count caps.
Test maximum legal field sizes, including envelope/encoding overhead. Thread
message/activity caps remain, with encoded-size limits added where needed.
New params are sent only to capable bridges. Legacy full-result forms stay
available in 3.x and appear separately in timing reports; they are not
quietly truncated, silently capped or labelled compliant.

An observation read returns its latest authoritative `value`, revision,
observed time and freshness. A cold missing observation returns explicit
`state: "pending"` with no fabricated empty value; a failed refresh retains
the last value plus the real observation error. Watch/subscription interest,
startup producers or a new journaled `observations.refresh` command schedule
work; the read itself remains pure. The refresh ack contains its job ID;
result publication updates the cache through normal events. File/Git body
pages bind to a stable revision and reject mixed-version assembly. Interest
and retained body caches have bounded quotas and eviction. Before confirming
a file/Git/review write, advance every affected observation fence: publish
new values or mark old ones stale. A post-confirmation current-value read
must return that revision or explicit pending, never a pre-write cached value
presented as current. Retained stale display data cannot remove an accepted
overlay; this applies to `fs.read/tree`, Git status/diffs and review surfaces.

The SPA continues drawing the previous cached value during refresh; a cold
surface derives its missing-content state from the cache, never connection
status. Fresh canonical SQL/snapshot values remain the default where already
cheap. This is the settled freshness/API scope in protocol §8, not permission
to pass the read gate with “pending” placeholders:
report cold-job completion separately, and gate ready-value reads separately
from bounded observation-status responses.

### Current SPA and MCP reads must be bounded

`spa/src/core/cacheSync.js::readTasksNow` has an unbounded `tasks.list`
fallback near baseline line 729, but line 727 already routes capable bridges
through `readTaskPagesNow`. Reuse that implementation: `trackerPages.js`
sets `TASK_PAGE_LIMIT = 100`, `pageParams` sends the limit/cursor, and
`tasks.listPaged` has existed since wire 1.25.0. Do not wait for a new
`reads.bounded` flag to use this existing paging capability. Apply the same
dispatch rule to `syncTaskPhases.js::projectPhases` and
`trackerTasksPane.js::readList/readWholeList`.

Before authoritative capability negotiation, keep cached paint and defer
the wire pull rather than treating unknown support as permission for an
unbounded fallback. Against a capable current bridge, every first and
continuation `tasks.list` request must send a positive measured limit,
preserve project/filter identity and follow opaque `next_cursor`; never
retry a failed page without its limit. Reserve whole-list compatibility for
confirmed old bridges/old clients, outside the current-pair qualification.

Keep existing page/cache semantics: persist each page, fold its authoritative
number stretch, paint page one immediately and continue the rest in the
background. Preserve rows outside the stretch and newer pushes/accepted
overlays. Follow a short/empty page when it has a cursor; an invalid or
repeated cursor is incomplete/error, not end-of-list. On epoch change or
cancellation stop the old walk and retain cached rows; remove obsolete page
records only after verified exhaustion. The 100-row limit does not bound
embedded bodies: the negotiated total-response ceiling/result references
above remain required for maximum-size records.

Step 8/9 acceptance exercises **actual current SPA and MCP dispatch**, not
just synthetic small RPCs. Every read they issue against the capable bridge
must belong to the bounded complete-result or explicit cached-observation
cohort. Add outgoing-request assertions and large-project, filter/empty-page,
concurrent-write, partial-walk, cold-greeting, capability-loss and epoch-change
cases to `trackerPagesWiring`, `syncTaskPhases`, `cacheSyncForeground` and
`trackerTasksPaneReadOrder` tests, and equivalent MCP adapter tests. Gate
each real page's full latency at p99 ≤100 ms; report first cached paint and
whole-project sync duration separately. A current-capable call taking the
unbounded compatibility fallback fails qualification.

### Measurement and release gate (step 9)

Define **server read latency** from carrier intake (including decryption and
admission) to the sealed **final** reply enqueued for the carrier. Include
queue/pool wait, SQL/I/O, projection, typed validation, serialization and
encryption. For MCP, use complete control-frame arrival through encoded
reply flush, including authentication/admission. Receipt/ack latency is
never read completion. Current `FrameTimer` handler totals end before
`SessionSender::seal`/enqueue; extend measurement at that boundary and record
each caller, including folded callers, rather than only shared computations.

Record browser/MCP caller latency separately from dispatch through completed
response: include SPA sender-credit wait and output/network transfer. A fast
server metric must not hide a seconds-long unsent credit queue. Report read
checkout/query/encoding intervals to diagnose breaches; do not subtract
percentiles. Existing eight-sample per-verb p95 evidence cannot establish p99.

Required evidence:

1. Pin revision, toolchain, host/CPU/storage, page sizes, fixture cardinality,
   client count and concurrency. Use temporary file-backed WAL data and
   encrypted fake carriers; no installed bridge or real services. Start with
   two project fixtures: 100 and 10,000 tasks, 10 and 200 workspaces, 1,000 and
   100,000 thread items, plus maximum-valid pages, large directories/diffs and
   fake slow providers. This is a declared test envelope, not a production
   frequency claim.
2. For every supported ready-value read method and MCP equivalent, collect
   at least 1,000 requests per scenario in three runs. Require **p99 ≤100 ms**
   in each run/scenario, including mixed hot writes and independent Git work.
   Report p50/p95/p99/max, sample counts, errors, cancellations and every
   >100 ms sample. No dropping slow/failing samples; valid success workloads
   must have no errors. Report pending-observation replies, refresh time and
   legacy unbounded calls as distinct cohorts, never blended into fast reads.
3. Exercise warm/cold caches, foreground/background mixes, eight concurrent
   reads per opening across four clients, journaled writes, SQL writer
   contention, saturated ordinary dispatch/Git workers, and bounded read
   bursts. Measure checkout pressure; tune batching/pool/query work before
   increasing pool size. Infinite held read connections/legacy floods are
   overload tests, not a false 100 ms promise.
4. Deterministic isolation tests hold app/writer/compatibility/Git barriers
   before read admission and again before completion. All independent
   snapshot/SQL final replies must arrive **within 100 ms and before releasing
   the unrelated barrier**, including MCP; use a suitable pinned timing
   runner and record actual maxima. Slow injected external providers must
   not prevent ready cached reads. No app/domain/writer lock acquisition is
   allowed even when it happens to finish within budget.
5. Retain bounded memory/WAL and read-pool backpressure tests; no read capacity
   `busy`, no app/writer fallback, no old-query folding after confirmation,
   no stale generation publication. Reads after confirmation see applied
   state; reads after ack retain the local accepted overlay without waiting.
6. Run the current SPA/MCP caller tests above, including
   `cacheSync.js::readTasksNow`, against the capable bridge. Assert that every
   task-list page includes its limit and that all current client reads use
   the qualified bounded/cached forms. Do not put these calls in the legacy
   cohort to obtain a passing result.

The settled target is p99 for bounded complete results in the declared
workload, plus the controlled maximum for lock independence. Arbitrarily
stalled storage, legacy unbounded responses, indefinitely held read workers
and WAN transfer are reported separately; they do not redefine this target.
A 100 ms timeout or capacity error does not satisfy it, and the current SPA
may not rely on an unbounded call to evade qualification.

## 8. Maximum write lock/admission wait: release gate

**Required at both the #709/step-5a handoff and step-9 final acceptance:
no admitted RPC or MCP write may wait more than 5,000 ms on locks or
admission.** This is an observed maximum, not p99. Keep one logical-write
identity and measure `write_blocked_ms` as the union of non-overlapping wait
intervals from RPC intake or complete MCP-frame arrival through execution
and publication completion. Each individual interval **and** cumulative
blocked time must be ≤5,000 ms, with zero outstanding overdue writes.

Include carrier/dispatch/blocking-pool queues, MCP `Turn::take` and
reacquisition, app/domain/compatibility/owner/native-conflict waits, SQL
admission/writer queues, post-ack executor queues, predecessor waits and
completion/publication re-entry. Instrument raw locks too, including
`DeliveryRunner::drain`'s current `state.lock()`. Count waiting behind another
command's external work; report the command's own active external I/O/work
separately. Ack does not stop/reset the meter. Repeated short waits cannot
evade the cumulative bound. A moved/renamed queue, timeout, cancellation or
discarded unfinished request cannot turn a violation into a passing sample.

### Declared qualification workload

Use §7's small/large temporary WAL fixtures and pinned host/toolchain, with
four SPA openings and eight MCP agent sessions across four projects and
eight workspaces. Run three ten-minute trials at 40 semantic writes/second,
up to two outstanding writes per caller, including an eight-write same-owner
burst and interleaved sibling-owner commands. Cycle the deployed RPC/MCP
write families: 80% bounded SQL/state updates, 10% injected filesystem/process
work of up to 250 ms and 10% injected Git/provider work of up to 500 ms;
record actual method mix and payload bounds, without real providers. Add
four independent background producers, the §7 read load, and 100 exempt
terminal/subscription/control frames per second. Separately exercise every
rare write family and deferred completion path with the same finite service
bounds; no method is excluded because the common-path mix missed it.

Require **no overload refusals or timeouts in this supported workload**,
complete all admitted work, and report per-method sample counts,
admission/lock components, cumulative/max waits, throughput and every
unfinished operation. Include both same-owner FIFO/dependent chains and
different-owner progress. Retain one logical identity through nested
`mcp.control`/delivery work. A watchdog fails the gate as soon as an active
write's blocked time exceeds five seconds; completed-request histograms
alone are insufficient. Record the same evidence at 5a and at final
acceptance; the journal cannot defer qualification of #709.

### Overload and fault cases

Run separate tenfold-burst and deliberately stalled-service/held-exclusion
cases. A bounded front door may return an explicit retryable write `busy`
**before durable acceptance**, promptly (test ≤100 ms refusal delivery under
the injected fixture). Report these refusals and offered/admitted rates
separately; they are not successful writes or evidence of low lock wait.
Do not accept work merely to abandon it at five seconds or acknowledge it
before its durable record. Accepted commands keep their recovery obligations.

Backpressure/admission must bound accepted work, including owner queues;
dropping commands or timing out a client does not fix contention. A sustained
fault can make completion impossible, but its >5 s waits are explicit fault
failures, never passing performance evidence. An intentional six-second lock
or admission stall must make the measurement/watchdog fail, both before ack
and after ack, proving that no wrapper or new queue hides the breach. Fix
the lock/service boundary or overload admission before qualifying the normal
workload. Read capacity refusals remain forbidden.
