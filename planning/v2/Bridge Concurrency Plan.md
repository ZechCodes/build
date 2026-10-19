# Bridge Concurrency Plan

Task #681, phase 1 — proposed design, 2026-10-19. **Planning only; no runtime
change is included.** Code reviewed at `8257cba8bc737b622978ccda3c20e43b7feb9222`.
Full paths are repository-relative. In explanatory text, `timing.rs`,
`carrier.rs`, `store.rs`, `api/` and their children are under `bridge/src/`;
`tracker/`, `conversations/`, `projects/`, `workspaces/`, `config/`, `board/`
and `runtime/` are under `bridge/src/app/` unless explicitly qualified.
The per-verb appendix supplies full paths. The accompanying amendment in
[`ARCHITECTURE.md`](../../ARCHITECTURE.md#app-state-and-its-lock) distinguishes
the current implementation from this target.

The outcome is independent read execution, SQLite outside application state
locks, and writes owned by the device, project, or workspace they actually
change. Merely widening the dispatcher, changing the app mutex to an RwLock,
or moving the existing Store mutex out of AppState cannot deliver that outcome.

## 1. Contract and evidence limits

The task settles five requirements: SQLite-only work does not wait on
in-memory/git work; read-only requests do not wait for application writers or
other readers; sibling writes proceed independently; dispatch does not queue
reads behind slow work; and a read issued after a successful write reply sees
that write. A concurrent read may describe an earlier committed state. Pushes
correct the SPA's cache; neither receipt admission nor an eventual push is a
substitute for the write's final reply.

“Never block” means no application lock, waiting connection checkout, shared
write worker, or queued older read on the admitted read path. It cannot mean
zero CPU scheduling or storage latency. All pools remain bounded. When a read
cannot obtain execution/connection capacity immediately, return the existing
retryable `busy` refusal, rather than accepting it into an unbounded wait.
This overload behavior needs explicit tests and measurement: frequent `busy`
responses would fail the usability goal even if locks disappeared. Cheap
snapshot reads and diagnostics have capacity independent of SQL and file/git
reads. A finite pool cannot promise successful completion of unlimited
simultaneous reads.

Similarly, one SQLite writer necessarily serializes SQLite transactions.
Independent workspaces may prepare and perform independent external work in
parallel, with short commits serialized at SQLite. Distinct logical workspace
IDs can also reach the same physical repository; shared Git refs still need
canonical repository exclusion. Neither fact justifies holding an app or
project mutex while waiting for them.

No live daemon, production database, browser, network configuration, pairing,
installer, or bridge executable was used for this plan. The task reports
#672 reduced a board lock hold from roughly 6 s to 0.4 s; that is **reported
context, not a reproduced measurement**. No production frequency distribution
or dominant-verb ranking can be established from this checkout. Earlier
`Bridge Concurrency Spec.md` / `Bridge Concurrency Primitives.md` statements
that SQLite is sub-millisecond or safe under the app lock are not evidence
for this task and are superseded by this proposed lock policy.

## 2. Where time goes today; measurement first

### Actual request path

`carrier.rs::FrameIntake::accept` decrypts/adopts through the carrier registry,
answers ping, sends an admission receipt, then normally hands the frame to a
per-wire admitter (capacity 1,024). `admit_in_order` waits to put it into
`carrier/dispatch.rs::Dispatcher`. The dispatcher has two queues of 256 and
eight worker tasks, of which at most two execute background work. This is
**not** a global eight-handler limit: terminal lanes, immediate signaling and
attachment handling, session teardown, MCP, and background services also run.
Any string `params.term_id` currently selects a serial `(session_id, term_id)`
lane (capacity 64), independently of the pool.

`run_handler` uses `spawn_blocking`; `FrameHandler::run` starts the timer
inside that blocking closure. `app/rpc.rs::dispatch_frame` checks update
admission, tries `session_scoped`, then `routed`. Ordinary typed dispatch is
`timer.lock(state).dispatch_deferring` → `AppState::route` →
`api/v1/mod.rs::dispatch`, whose `Handler::call` takes `&mut AppState`.
Deferred Git work releases the app mutex, but keeps its dispatch worker until
`apply_deferred_stage` and the reply finish. Most reads therefore still queue
and take the same app lock. Even the update admission lookup and project
deletion check can take it before the handler.

`Store` clones share `Arc<Mutex<Connection>>`. `Store::new` enables WAL,
`synchronous=NORMAL`, foreign keys, and a five-second busy timeout;
`in_transaction` uses an immediate transaction. Reads use the same connection
mutex as writes. A Store clone is not a read connection. WAL alone cannot
create concurrent reads through that mutex. `off_the_workers` moves blocking
sections off Tokio runtime workers; it does not remove serialization or bound
all blocking submissions.

### Existing instrumentation

| Component | What code actually measures | Missing or misleading inference to avoid |
| --- | --- | --- |
| `timing.rs::QueuedFrame::{start,promote}` | Queue depth and elapsed time from `Dispatcher::dispatch` until the blocking handler starts | Omits per-wire admission wait; conflates dispatch wait and Tokio blocking-pool wait |
| `FrameTimer::lock`, `LockedFor::{drop,wait_until}` | Sum of instrumented app-lock waits and holds; condvar waiting counts as wait, not hold | Bare locks in services/facts are not automatically attributed to the requesting verb |
| `FrameClock::stats`, `MethodRecord::stats` (`bridge.stats`) | Per-method served computations, total p50/p95/max, held p99/max; queue depth, priority totals, current holder | Queue `p95_ms` is total frame latency by priority, **not queue-wait p95**; no phase percentiles or time sums |
| `slow_frame_line` | Total, queued, lock_wait, held, waiting depth; optional permit_wait/pool_wait | Threshold 200 ms, production throttle once/method/10 s; not a complete trace; depth is observed when the timer publishes |
| `app/mcp.rs::Turn::take_again`, `answer_off_the_workers`, `apply_off_the_socket` | MCP semaphore and blocking-pool waits | These fields are not wired for ordinary carrier RPCs |
| `Store::connection`, `in_transaction` | No timing | Cannot separate connection mutex, SQLite busy wait, SQL execution, commit, or checkpoint from app hold |
| `run_folded_read` | One timed computation for queued twins of board.list/project.list/git.status/git.log | Served count is not caller count; each folded caller's wait is missing |
| `bridge.stats` | Handler reads FrameClock without app lock | Still sits in ordinary admission/dispatch queues; not a diagnostic fast lane |

The histogram has 18 fixed buckets and tracks only 128 method names before
collapsing into `other`. There are already 138 v1 verbs plus legacy, QA and
internal names. Step 1 must reserve bounded records for registered/internal
names and collapse only unknown names. Do not add entity IDs, paths, SQL text,
user content, or arbitrary labels to metrics.

Handler total also omits portions of intake/decryption and reply
serialization/encryption/delivery. Never subtract independently computed
percentiles to infer SQLite or queue cost. Lock hold contains SQLite today;
those durations overlap and must not be added twice.

### Baseline experiment to ship first

Build a library test/benchmark harness with private temporary paths, injected
shell/harness/service dependencies, synthetic carrier frames, local test
repositories and fake slow jobs. No daemon binary or existing user state.
Exercise `FrameIntake`/`Dispatcher`, not only `FrameHandler::call`, which
bypasses the queues. Pin fixture seed, schema, repo/task/agent/message counts,
runtime workers, pool limits, concurrency, sample count and revision in its
report. Separate cold and warm runs; use a fresh clock for each run.

Collect raw request samples plus `bridge.stats` and captured slow lines for:

1. Each candidate verb alone: `board.list`, `project.list`, `workspace.list`,
   `workspace.get`, `agent.list`, `thread.page`, `thread.activity`,
   `tasks.list`, `tasks.get`, then representative Git and writes.
2. The same reads during a gated workspace write, slow Git operation, held
   app mutex, held SQLite writer, and long SQL read, independently. Saturate
   every write slot, not just one, to expose worker starvation.
3. Same-scope and distinct-scope writes; same project/different workspace,
   different project, and different logical scopes sharing a Git common dir.
4. Concurrent post/page/reset, task update/list, subscription changes, session
   close/reopen and background pump/service traffic. Include first cache fill.

Record admission, dispatch, execution-pool, scope wait/hold, DB checkout,
SQLite execution (including internal busy handling), transaction commit,
publication, reply preparation, and total separately. Record SQL call count,
returned rows/bytes, folded callers and overloads. Exact separation of SQLite
busy handling from statement time needs an explicit supported hook; until
implemented report it as included, not as zero. Collect per-phase sums and
p50/p95/p99/max; distinguish exclusive wall-clock phases from nested spans.

Rank verbs by request count, summed app hold, summed SQL duration, and summed
queue wait, with sample counts. The initial hot-path order below is a
**candidate order based on task context and code fan-out**, to be reordered by
this baseline. The plan does not claim measured production dominance.

Deterministic gates establish concurrency: the read must finish before the
held write is released. Timing thresholds alone do not prove that property.
Use generous failure deadlines only to detect hangs. Performance budgets and
pool sizes are selected from the baseline, not invented here.

## 3. Target ownership and read design

### Stable service handles; immutable published views

Introduce `AppServices` outside `Mutex<AppState>` with handles to `StoreRead`,
`StoreWrite`, the publisher, scope coordinator, scheduler, FrameClock,
update-admission service, ChangeBus, peers and session services. Construct it
before accepting requests. `AppState::handler` captures those handles once;
read dispatch never reacquires the legacy app lock just to find them.

Publish an immutable `ReadView` through an atomic Arc publication abstraction
with no reader acquisition of a writer-held lock. Do not replace the app mutex
with one `RwLock<AppState>`: readers would still wait on writers. Published
values contain wire-oriented data and stable scope/generation mappings, not
mutable `ActiveRun`, PTY handles, `&Store`, or interior-mutability backdoors.
Share unchanged project/workspace chunks by Arc; bound thread tails, avoid
cloning entire histories or rebuilding the whole board on every token.

The write owners hold mutable device, project and workspace state separately.
Device-owned services include settings, user-session activity, client
subscriptions, upload admission quotas and service snapshots. Project owners
hold sources, task policy and project conversations; workspace owners hold
workspace registry entries, runs, conversations, tabs, delivery and reclaim
state. Registry membership, ownership indices and summary projections are
published deltas, not a reason for every workspace to take a device/project
exclusive lock. `AppState` remains a compatibility adapter during migration.

Compatibility is an explicit gate: an unmigrated writer must reserve the
affected scopes too, or hold a conservative legacy-exclusive reservation
that conflicts with migrated writers. Introduce that gate before enabling
scoped mutations, including background producers. Never run an old
uncoordinated `&mut AppState` mutation beside a new scoped writer. A family
loses this fallback only after all its helpers and producers are audited.
Read paths already migrated remain independent of the compatibility gate.

`ReadView` contains ready board, project, workspace, agent and terminal rows,
settings/service snapshots, canonical scope indices and bounded live thread
metadata. A board read must not call `work_items`, reconcile attention, walk
Git, or reload workspace manifests. Mutations and maintenance services update
these projections. Local capability/remote/volume observations are refreshed
outside reads; warm them before initial publication where the wire requires
a value. Preserve existing scanning/optional-field behavior rather than
inventing a loading state or returning an empty board on a cache miss.

Move `session_seen` and `session_summaries` updates to append/reset/recovery
publication. `session_seen` is the per-owner/per-agent sequence already
observed for session summaries (`session_summaries.rs::note_run_messages`),
not the user's read cursor. Keep explicit `entity.seen`, `tasks.read_through`
and `user.present` as writes. Reads do not synthesize human activity.
`USER_ACTIVITY_VERBS`/`INTERACTION_VERBS` remain explicit policy lists.

`tasks.list/get` backfills, lazy tracker cursor-key creation, parked-project
revival, legacy-workspace adoption, board attention reconciliation, vanished
run sweeps, and first-cache-fill claims become startup/background commands.
A read may enqueue a bounded, coalesced maintenance hint without waiting;
the service owns the mutation. Backfill-derived values can be computed
purely for the returned row until persistence catches up. Errors never
silently turn unavailable canonical data into a successful empty answer.

### SQLite ownership

`StoreRead` lends an idle read-only connection immediately to one read job;
the job uses a short read transaction and releases statements and the
transaction before returning the connection. No connection is concurrently
shared; no read takes the writer's Rust mutex. Initialize/migrate with the
writer before opening the read pool. Configure read-only/query-only behavior
explicitly, verify WAL on a file-backed temporary database, and keep foreign
key and schema checks. Do not run migrations from a reader or use a writable
connection as a pool fallback. Preserve Store-free test behavior explicitly.

`StoreWrite` owns one writable connection on a dedicated worker. Typed
transaction commands carry owned inputs, expected revisions and an operation
identity; they return committed data/deltas. The worker never calls AppState,
waits for a scope, runs Git, writes config/manifests/attachments, or invokes a
publication callback while a transaction is open. Preserve the current
immediate-transaction reasoning and commit-before-wakeup behavior. Statement,
commit and checkpoint durations are instrumented. Backup/migration/recovery
ownership also moves through this boundary; this plan runs none of them.

No app/domain mutex is held while enqueueing, awaiting or executing SQL.
Legacy callers are converted by family; an async wrapper around
`Store::connection` followed by `.wait()` under AppState is not conversion.
Keep a bounded writer queue and return admission errors before side effects
when it is full. Read pool exhaustion refuses promptly; a long reader pins
only its own WAL snapshot. Bound query work/pages, observe WAL growth, and
schedule checkpoints on the writer. Do not put a blocking checkpoint or
five-second read busy timeout on a read that is advertised as independent.
External SQLite contention remains possible and must surface as a bounded,
measured refusal rather than a silent app-lock hold.

For canonical task/history reads, resolve the request's immutable scope
mapping and perform related SQL reads in **one request transaction**: task,
timeline, review/dependency summary and retained identities cannot come from
separate arbitrary pool leases. Persisted tasks use `project_path`, while
wire replies use project IDs.
Despite the stale boot-local-ID comment in `tracker/views.rs`,
`config/persistence.rs::config_value_with_project` and
`projects/mod.rs::restore_configured_projects` save and restore configured
IDs. Keep an immutable path/ID mapping with deletion/source generations;
neither path identity nor current membership follows from the ID alone.
`tasks.list`'s live agent availability/user-session overlay is
explicitly eventually consistent with an in-flight write. Persisted fields
and timeline must agree transactionally.

Thread reads need stronger preparation than cloning a resident tail. Persist
or expose transactional canonical owner/conversation/thread-generation
metadata beside the history; resolve and validate it inside the same read
transaction. After reset, never combine a pre-reset tail with post-reset SQL.
For configured stores, serve canonical bounded pages/activity/operation
receipts from SQL, including summaries needed to preserve the wire contract;
for Store-free execution, use one immutable conversation generation. Preserve
lean-activity digests and paging limits. The current resident/SQL merge
helpers are evidence of a migration problem, not proof they are safe off-lock.
Whether existing schema columns suffice for all these lookups must be
established in step 4; add a migration if they do not.

Do not promise a single transaction spanning SQLite, all in-memory views and
the filesystem. A combined response must choose an authority for each
invariant: transactional durable rows from SQL, or a completely published
projection. Optional live observations may lag; IDs, generation fences,
task/timeline consistency and mutation preconditions may not be joined from
incompatible generations. Reads need not wait for a global publication epoch.

### Scoped writes and lock order

Resolve scope from trusted canonical identities, not arbitrary parameter
paths. Upload IDs resolve through their owner session and saved scope;
conversation IDs resolve to their project/workspace; source aliases resolve
to a canonical Git common directory. Project conversations and original
source terminals are project-scoped; adopted standalone runs need a stable
workspace-equivalent owner before they migrate. Cross-scope work declares
the union, including destinations and retained task/conversation updates.

Proposed lock/ownership order:

1. **Device → project → workspace**, ascending canonical IDs within each
   level. Use compatible ancestor intention reservations for child writes;
   do not exclusively lock the device and project for every workspace write.
   A workspace A write and workspace B write in the same project can both
   hold their ancestor intentions. A project deletion takes an exclusive
   project reservation and waits for its descendants, not other projects.
2. Acquire the complete scope reservation set all-or-none in a short
   coordinator section; release the coordinator's mutex immediately. Waiters
   wait outside it. No lock upgrades, and no retaining a partial set while
   waiting for an earlier or later key. On discovering another scope,
   release, recompute and revalidate. Give queued structural/exclusive
   operations fairness without making readers participants.
3. Inside the owned scopes, briefly lock mutable records to validate and
   construct an owned command/projection. Release all state guards for SQL,
   file/Git/process work and publication. An operation reservation/token may
   remain to exclude a conflicting writer; it is not a held state mutex and
   is never acquired by a reader or the SQLite worker.
4. External Git exclusion is by sorted canonical common-dir identity, after
   scope reservation and outside state guards. `SyncLock` remains necessary
   for aliases/linked worktrees and source sync. No code holding it may
   reacquire a scope/state mutex: validate using captured generation tokens
   or the published view, then release Git locks before settlement. Existing
   `git/deferred.rs::ScopedGitCall::run` takes SyncLock and then AppState;
   remove that reentry before installing the new order. Audit source-sync
   preparation/revalidation and review callbacks for the same inversion.
5. SQLite commands execute with no state/Git lock held, return after commit,
   and release the connection before applying results. Multi-stage external
   operations reserve/journal, release DB, do I/O, release Git lock, then
   commit settlement. They are sagas with explicit recovery, not an imagined
   SQL+filesystem transaction.
6. Publish immutable deltas, then note ChangeBus events and enqueue delivery,
   then reply. No bus, screen, subscriber, metrics, writer, or publisher
   internal guard calls back into scope/state acquisition. Resolve handles
   first and use leaf services after state guards drop. Preserve the ban on
   holding a blocking guard across `.await`.

A SQL-only command can omit application scope reservation when its entire
invariant is checked in the writer transaction (for example a key record).
This avoids coupling unrelated SQL to filesystem owners. A command combining
SQL with live state needs the reservation and expected-version protocol.
Writer commands never depend on publication acknowledgement before allowing
the next unrelated DB command. Thus SQLite is not downstream of an app lock.

Keep a mutation's operation reservation through publication and notification
staging, not merely through SQL commit. Otherwise two writes to the same
scope could publish in reverse order. New SQL-only mutations that affect
published data still need a per-record version and settlement ordering;
“no scope reservation” does not permit unordered projections.

Reservations retain generations across deferred work and terminate on
success, failure, panic and cancellation before durable acceptance. After a
durable accept/commit, a request disconnect must not cancel settlement: an
owned operation record/job completes publication and cleanup. A late job
cannot recreate a deleted workspace, clear a replacement tab, spend an old
completion authority, or overwrite a newer source path. Use existing
operation receipts, review versions, source-location checks, reset guards,
reclaim reservations and assignment generations as requirements to preserve.

Device-wide user activity must not force every workspace write through a
long device lock. Publish an ordered timestamp observation through its own
small service after successful action. Preserve its current best-effort
persistence semantics; if the primary action is SQL-backed, include the
activity update in its transaction where possible. For non-SQL actions,
persist observations asynchronously, without making a Git/terminal command
wait for unrelated database work. `user.present` and subsequent task reads
must observe the published session before the presence reply is acknowledged.

### Dispatch lanes and lifecycle

Add registry metadata for effect/read source, scope resolver and executor.
Unknown/unmigrated verbs remain on the conservative legacy path; never infer
read safety from prefixes, method names or the deletion allowlist. A test
compares all v1, legacy and QA entries against the metadata table.

Classify **before the per-wire blocking admitter**. Separate diagnostics,
snapshot reads, SQL reads, file/Git reads, writes, and maintenance. CPU reads
and SQL reads do not share the blocking pool saturated by Git/write work.
Keep foreground/background fairness inside the applicable lanes; a client's
priority cannot move a write into a read lane. Fast intake only validates,
receipts and schedules; it does not execute SQL or serialize a large board.
At capacity, immediate refusal prevents an older write/read from holding up
the wire's later cheap request. Stats gets reserved diagnostic execution.

Admission permits must correspond to available executor/connection capacity,
not merely a slot in a second hidden queue. Verify the SPA's existing handling
of retryable `busy` before enabling this policy; automatic useful retries are
not established by this backend audit. If it needs changes, add bounded,
jittered retry in the RPC/cache layer without making views connection-aware,
and test refusal storms/cancellation. Do not claim success from latency
percentiles that omit refused requests.

Keep terminal input/resize/ack/attach/close ordering and session-opening
ownership. Restrict terminal-lane classification to the supported ordered
verbs instead of letting an arbitrary `term_id` force a normal read into a
terminal queue. Any compatibility change here needs contract tests. Carry
`SessionSender` explicitly across async/off-thread work; the thread-local
`changes::with_session` must not leak into another request.

Preserve receipts, one terminal reply per caller, cancellation-safe folding,
per-caller response wires and close/reopen fences. Folding may combine only
queued reads whose snapshot/consistency floor satisfies **every** caller;
never attach a read issued after an acknowledged write to an older running
read. Nonqueued fast reads generally need no folding. Track each opening's
accepted work separately from mutable app state; teardown marks it ended,
prevents new effects, and lets already durable writes finish without attaching
resources or publishing session-specific pushes to a replacement opening.

MCP `Turn` scheduling, delivery workers, pumps, facts, reclaim, source sync,
watchers and update callbacks must use the same ownership rules. Optimizing
browser dispatch alone leaves autonomous services holding the old mutex.
Keep dedicated liveness/push runtimes and `off_the_workers` for blocking work;
do not substitute an unbounded `spawn_blocking` per read for a bounded design.

### Read your own write, publication and pushes

For a successful mutation, order **durable commit → publish affected read
views/invalidations → enqueue change/delivery events → send final reply**.
A lifecycle acknowledgement may mean durable acceptance rather than completion
of Git/provisioning; its next read must show the accepted/pending record,
not pretend the asynchronous work is already done. Keep that distinction
from transport admission receipts.

The publisher applies versioned per-scope deltas to the latest immutable
root, merging disjoint changes instead of replacing the entire root captured
before a write. Multi-scope effects appear in one published root transition.
Prebuild projections outside its short commit/CAS section; retries contain
no SQL, filesystem work or callbacks. Do not let a late refresh or earlier
commit regress an entity version. Maintain deletion tombstones/generations.
If commit succeeds but publication cannot complete, retain the settlement
job, expose an unknown/recoverable outcome rather than acknowledging success,
and rebuild from durable records after restart. Never “roll back” only memory
after a committed database write.

Because the reply is held until publication, a subsequent snapshot read
loads a view at least as new as that write; a fresh SQL transaction sees the
committed rows. This satisfies read-your-own-write without new client tokens
or a per-session read barrier. Persisted write retries retain operation IDs
and current conflict behavior. Old SQL transactions may not survive pool
return; response caches/coalescing must use current scope revisions. Git/file
reads after a successful mutation must use a fresh observation or a refreshed
projection of that write, never an invalidated old cache. External edits after
the acknowledgement can of course change the result again.

The guarantee concerns committed canonical fields and their affected
canonical projections. Live telemetry may lag a concurrent operation, but
must match canonical identity/generation and must not override it. Discard
a mismatched live overlay; do not silently splice a newer memory tail into
an older SQL page. Existing operation receipts are not universal: writes
without durable replay identity need one, or an explicit recoverable
settlement record, before they use the commit/publication split.

For the first implementation, keep existing wire shapes and capabilities.
No client-required epoch is proposed. Internally tag projections/change
work with generations so pull replies, delayed refreshes and push assembly
cannot regress committed state. `changes.rs::ChangeBus` retains its initial
flush and 250 ms coalescing behavior. `app/facts.rs::entity_facts` reads
published views and read-only SQL, not `Weak<Mutex<AppState>>`; it drops bus
locks before work. A push contains data from at least the recorded committed
change or a refetch indication when the existing contract permits it.
Do not emit a successful state change for a failed transaction. Partial
external work that is durably journaled can emit its actual failed/partial
state. The writer releases its transaction before notifying waiting-on or
delivery services. Push construction does not delay mutation acknowledgement
for the coalescing window or network delivery.

## 4. Implementation sequence and TDD gates

Each step is a separately reviewable, shippable change. Preserve the old path
for unmigrated families and explicitly report which guarantees the step does
and does not yet provide. Write each behavioral test first and observe its
failure. New filenames/functions below are proposals, not claims they exist.
Do not mark the complete concurrency contract shipped until step 9 passes.

### Step 1 — Complete measurements and make safe fixtures

Change `timing.rs::{FrameClock,FrameTimer,QueuedFrame,MethodRecord,LockedFor}`,
`carrier.rs::{Admitted,FrameIntake::accept,admit,answer_without_queueing}`,
`carrier/dispatch.rs::{FrameHandler::run,run_handler,run_folded_read}` and
`store.rs::{connection,in_transaction}`. Carry an explicit timing context into
Store work, including deferred/MCP paths, without global thread-local
attribution. Extend `app/mcp.rs::{Turn::take_again,answer_off_the_workers,
apply_off_the_socket}` consistently. Add bounded metrics for scope ownership
in preparation for step 6; do not reuse the single `lock_holder` slot for
multiple locks. Preserve older stats keys and update
`fixtures/api/v1/bridge.stats.json` / contract expectations if applicable.

Add `test_support/concurrency.rs` (proposed) and explicit test dependencies in
`AppState::new_with_context`/construction helpers so tests never resolve user
homes, shell environment or live service binaries. Add a private-data
`tests/request_concurrency.rs` harness rather than reusing the existing PTY
load test unchanged. Record the baseline described in §2 and reorder the
following hot-family migrations from its results.

Tests: existing pure timing/dispatcher tests; deterministic phase attribution
including admission and RPC pool delay, nested SQL spans, commit failure,
folded-caller counts and unknown-method cardinality. Saturate all ordinary
workers and demonstrate the **current** read/diagnostic queue wait. Golden
fixture counts and identical seed before/after. This step improves diagnosis,
not concurrency; it must publish raw samples and observed rankings.

### Step 2 — Typed effects, service handles and independent admission

Change `api/v1/mod.rs::{Handler,v1_method!,methods,dispatch}` and all twelve
family `methods()` tables; `api/mod.rs::{LEGACY_METHODS,QA_METHODS}`,
`app/rpc.rs::{AppState::handler,dispatch_frame,session_scoped,routed}`,
`carrier.rs::{FrameIntake::accept,admit_in_order}` and
`carrier/dispatch.rs::{Dispatcher::dispatch,run_handler,ordered_lane,read_key}`.
Add `app/services.rs`, `app/read_view.rs`, `api/effects.rs` and independent
executor/admission modules (proposed). Move service handle lookups, update
admission, peer/diagnostic access outside the app lock. Initially route only
proven snapshot/service reads (e.g. stats and immutable catalogs after
snapshot extraction); keep other methods marked legacy until migrated.

Tests: every registry/legacy/QA verb has one explicit classification; invalid
params/unknown verbs preserve errors; stats and a migrated snapshot read
finish while all old write workers and the app mutex are held; full write
admission does not obstruct a later read on the same wire; SQL/file overload
does not consume diagnostic/snapshot capacity; immediate busy at capacity;
terminal order, receipts, cancellation, session close/reopen and fold fanout.

### Step 3 — WAL read connections and the writer boundary

Change `store.rs::{Store,new,connection,in_transaction,
in_waiting_on_transaction}` and storage call sites incrementally. Add
`store/read_pool.rs`, `store/writer.rs` and typed read/write contexts
(proposed); extract SQL helpers in `store/{tracker,conversations,operations,
reviews,waiting_on,delivery_intents,user_session,push_keys}.rs` to accept
the appropriate connection/transaction instead of relocking Store. Keep
startup migration and backup paths explicit. Route the smallest canonical
read/query slice first, then the hot reads in step 4; legacy callers remain
compatible with the **same writer**, never an accidental second writer.

Tests with a file-backed temp WAL database: one held write transaction and
two independent reads complete; a held reader does not block another admitted
reader; the reader cannot write; each pool lease starts fresh after return;
pool capacity refuses without waiting; write atomicity/failpoint rollback;
unknown/newer schema refusal, startup migration, connection failure recovery,
post-commit waiting-on wakeup. Retain opposite dependency edge/cycle and
independent-connection workspace-claim tests. Instrument legacy DB time
honestly until its app-lock callers are removed.

### Step 4 — Task and conversation reads off the app lock

Change `app/tracker/mod.rs::{tasks_list,listed_rows,tasks_get,tracker_task,
backfill_done_at}`, `tracker/identities.rs::{backfill_task_identities,
task_with_read_identities}`, `tracker/pages.rs`,
`store/tracker.rs::tracker_list_key`, `tracker/waiting_on_api.rs` and
`tracker/reviews.rs::{review_get,review_diff}` with its projection helpers.
Move backfills/key initialization into a maintenance/startup job. Route
canonical queries through one readonly transaction; supply immutable
project/identity overlays. Refine error paths in `api/v1/tasks.rs` so they
cannot invoke a mutating `tasks_get` while formatting a refusal.

Change `app/conversations/read.rs::{thread_page,thread_activity,
thread_page_at,thread_page_after,stored_thread_page,stored_thread_delta,
lean_thread_page}`, `conversations/post.rs::thread_operation`,
conversation binding/revision readers and `store/{conversations,operations,
conversation_reset}.rs`. Add transactional generation/owner reads and a
schema migration only if the audited schema requires one. Update typed
`api/v1/{tasks,thread,reviews}.rs` handlers to receive read contexts.

Tests: task list/get and thread page/activity complete while app and workspace
write guards are held; two long reads overlap; task/timeline/dependency and
review tuples are transactionally consistent; list/get no SQL writes;
pagination/cursor keys stable across restart; legacy identity/done-at output
unchanged before maintenance; reset interleavings cannot mix generations;
bounded/newest activity digests and stored-history paging still match existing
fixtures. Read after acknowledged post/update/reset on another worker and
pool connection sees it. Store-free path parity and corrupted-store errors.

### Step 5 — Board/project/workspace/agent views and push facts

Change `app/board/views.rs::{board_list,work_items}`,
`board/cache.rs::{refresh_if_stale,publish_diff_refresh,apply_cache_publication}`,
`board_index/{mod,cache,attention}.rs`,
`projects/list.rs::{defer_project_list,project_list_rows}`,
`projects/parked.rs::revive_parked_projects`,
`workspaces/mod.rs::{workspace_list,workspace_get,adopt_legacy_workspaces,
resolve_workspace_directory}`, workspace capability refresh helpers,
agent roster readers, `config/settings.rs::settings_get`, `config/mod.rs::account_availability`,
`session_summaries.rs::{note_run_messages,replace_session_summaries}` and
`app/facts.rs::{FactsHandle,bus_with_sources,entity_facts,locked_facts}`.
Publish complete projections from mutation and refresh completions. Route
`api/v1/{board,workspace,thread}.rs` reads to them; adapt `changes.rs` fact
assembly. Retain existing reverse conversation lookup optimizations.
Before enabling a family's snapshot reads, cover its legacy mutation
completion hooks too: every acknowledged change to its canonical projection
must publish synchronously with settlement, even while the writer itself
still uses the old app mutex. Periodic rebuilding alone would violate RYOW.

Tests: all these reads and push facts finish with a write in progress; no
Store access/filesystem/Git/maintenance mutation from snapshot reads; first
publication and refresh failure preserve valid rows; deleted/superseded
workspaces cannot reappear from late refresh; two concurrent scope deltas
both survive publication; bounded tail/copy work; board attention/session
semantics and current JSON fixtures. Subscribe/write/read/push race and
coalesced events must never publish a state before its commit or regress a
generation. Run the #672 lookup-count regressions.

### Step 6 — Scope coordinator and common writes

Add `app/scopes.rs`, `app/mutations.rs`, `app/publication.rs` (proposed:
`reserve_all`, `prepare`, `commit`, `settle`, `publish_delta`). Split state
ownership out of `app.rs::AppState`. Change `app/transactions.rs::{
persist_plan_record,persist_run_record,finish_plan_mutation,finish_run_mutation}`,
`tracker/mod.rs::{commit_task_write,publish_task_write,tasks_create,
tasks_update,tasks_comment}`, explicit watch/read-mark handlers,
`conversations/post.rs::thread_post`, conversation watch/choice handlers and
`session_summaries.rs::{note_user_activity,user_present}`. These hot writes
use owned commands, version/receipt checks and commit→publish→ack, with no
app/domain guard across SQL. Adapt typed handlers and MCP tool entry points
for each migrated command in the same change.

Tests: distinct workspace writes and different project writes reach held
external-work barriers simultaneously; same-scope writes serialize; project
exclusive deletion waits for its own child intentions, unrelated project
continues; reverse-order multi-scope requests cannot deadlock; dynamic scope
discovery restarts rather than upgrades; no state-lock holder waits for SQL.
Store failure leaves no successful push/ack or half-updated memory; committed
but disconnected requests still settle; body/review/version preconditions and
operation replay remain correct; no lost publication deltas; acknowledged
write followed immediately by every affected read surface sees the write.

### Step 7 — Lifecycle, config, Git and file operations

Change `app/runtime/deferred.rs::{dispatch_deferring,take_deferred,
apply_deferred_stage,defer_read,defer_job,reserve_row,release_row}` so deferred
state is request-owned, not the single `AppState::deferred_work` and result
check slots. Change `app/git/deferred.rs::ScopedGitCall::run` and
`app/git/mod.rs::apply_git`,
project creation/deletion/source mutation and base-sync functions in
`app/projects/{requests,deletion,source_update,base_sync}.rs`, `workspaces/{mod,directories,deletion,
git_initialization,lock,reclaim}.rs`, `runs/` and `worktrees/` lifecycle
handlers, `config/` persistence and compaction handlers, `app/fs.rs` and
`fs/uploads.rs`, and
`app/tracker/reviews/{opening,publication,merge,lifecycle}.rs`.
Retain physical SyncLock protection; remove state-lock reentry from external
work. Replace the global project-deletion flag/filesystem count with scoped
reservations/counters. Serialize full-file config persistence in a separate
versioned owner to prevent lost concurrent project/workspace edits.

This is a family of small patches: Git/file reads first; simple file/upload
writes; workspace lifecycle; project/config changes; reviews/capture routing.
Each patch names the table rows it migrates and ships with its own tests.
New modules named as proposed must reuse these handlers rather than create
parallel business logic.

| Patch | Concrete functions beyond the common drain | Tests attached to this patch |
| --- | --- | --- |
| 7a: remaining I/O reads | `app/git/mod.rs::{git_log,git_status,git_diff,git_show,git_refs,git_unpushed,changeset_diff,worktree_diff}`, `app/fs.rs::{fs_list,fs_tree,fs_read}`, `app/tasks/documents.rs::{plan_doc,plan_stage_doc}`, `app/github.rs::defer_github_repos` | Held Git/file/CLI job cannot consume SQL/snapshot capacity; resolver does no adoption; unchanged path fences, errors, paging and deadlines. |
| 7b: Git/files/uploads | `app/git/mod.rs::{git_stage,git_commit,git_fetch,git_pull,git_push,git_stash,git_stash_pop,git_discard,git_merge_abort,git_checkout_ref,apply_git}`, `app/fs.rs::{fs_mkdir,fs_write,fs_create_directory,fs_create_file}`, `app/fs/uploads.rs::{fs_upload_begin,fs_upload_chunk,fs_upload_finish,fs_upload_abort}` | Canonical repository aliases serialize, independent repos overlap; RYOW after commit/file write; source movement; upload ownership, quotas, abort and close races. |
| 7c: workspaces/runs | `workspaces/mod.rs::{workspace_create,workspace_retry,workspace_finish,workspace_rename,workspace_ensure_conversation}`, `workspaces/directories.rs::{workspace_add_directory,workspace_remove_directory}`, `workspaces/deletion.rs::workspace_delete`, `workspaces/reclaim/explicit.rs::workspace_reclaim`, `workspaces/lock.rs::workspace_set_locked`, `workspaces/git_initialization.rs::workspace_init_git`; `runs/lifecycle.rs::{run_message,run_abandon,run_adopt}`, `runs/review.rs::{run_request_changes,run_git_action}`, `worktrees/dispatch.rs::branch_dispatch` | Same-workspace exclusion and sibling progress; reserve/work/apply failure and cancellation; deletion vs terminal creation; source-sync/reclaim guards, durable partial work and restart. |
| 7d: projects/config | `projects/requests.rs::{project_create,project_init_git,project_set_remote,project_add_source,project_remove_source,project_set_isolation}`, `projects/source_update.rs::project_update_source`, `projects/deletion.rs::project_delete`, `projects/conversation.rs::project_ensure_conversation`, `projects/base_sync.rs::project_sync_source`; `config/persistence.rs::{config_value_with_project,persist_config}`, `config/settings.rs::settings_set`, `config/compaction_wire.rs::{set_project_compaction,set_workspace_compaction}` | Project deletion fences own descendants only; concurrent config changes retained; shared-file persistence failures; parked IDs preserved; source move invalidates stale jobs. |
| 7e: reviews/captures | `tracker/reviews.rs::{review_snapshot,review_act,review_complete}`, `tracker/reviews/opening.rs::review_open`, `tracker/reviews/publication.rs::{review_push,review_update}`, `tracker/reviews/merge.rs::review_merge`, `tracker/reviews/lifecycle.rs::{review_close,review_reopen,review_refresh}`, `captures/mod.rs::{capture_create,capture_answer,capture_cancel,capture_reroute}` | Cross-scope destinations and canonical refs, version conflicts, replay/partial failure; capture routing releases device ownership before target dispatch; no callback lock inversion. |

Tests per patch: read/write isolation and RYOW for its rows; same physical
Git common-dir aliases serialize while unrelated repos overlap; source
movement and canonical path fences; no SyncLock→state inversion; upload
session ownership/quotas/chunk order/abort/reaper races; config concurrent
edits and persistence failure; rename/delete/reclaim against delayed Git,
source sync, terminal creation and review publication; partial journaled
failure/restart; existing review conflict/replay and generation tests.

### Step 8 — Session, MCP and autonomous producers

Change `runtime/terminals.rs::{session_hello,term_create,term_attach,
term_input,term_resize,term_ack}`, `runtime/agents/endpoints.rs::{
agent_attach,agent_start,agent_interrupt}`, session registry and reaper,
`runtime/pumps.rs` producers, `runtime/delivery/runner.rs::DeliveryRunner`,
`app/mcp.rs::{Turn,answer_off_the_workers,apply_off_the_socket}`,
`tracker/{waiting_on,delivery_service}.rs` services,
`workspaces/{reclaim,sizes}.rs`, project source-sync service,
`app/{updates,watchers,streams}.rs` and `api/v1/changes.rs::{
with_session,changes_subscribe,changes_unsubscribe}`. Replace whole-AppState
weak handles in services with scope/service handles; maintain explicit
blocking boundaries in `runtime/off_the_workers.rs::off_the_workers`.
No ignored producer may keep the old global lock alive as the real writer.

Tests: one-worker Tokio timer/I/O makes progress with each state lock held;
pump EOF/receipt/activity for an old instance cannot touch a replacement;
durable delivery and completion authority survive concurrent reassignment;
source-sync/reclaim idle services do not hold unrelated scopes; terminal
order, attachments and no-subscription-after-close; no harness launch in
fixtures; MCP read isolation under long writes and fair write admission.
Keep dedicated liveness/push runtime regressions and existing shutdown gates.

### Step 9 — Remove compatibility bottlenecks and accept the result

Remove remaining request access to `Arc<Mutex<AppState>>` and writer connection
guards from handlers/facts/services. Retire generic mutable v1 dispatch and
single deferred slots only when no callers remain. Update `app.rs`,
`app/rpc.rs`, `api/v1/mod.rs`, all table classifications, `store.rs`, scheduler
and `ARCHITECTURE.md` to describe what actually shipped. Add source/compile
checks ensuring readonly handlers cannot obtain write contexts. Historical
specs must point to the new lock policy rather than retain their exception
for SQLite under the app lock.

Tests: rerun the fixed-seed matrix and report before/after phase distributions,
throughput, overload rate and memory/WAL growth. All admitted read probes
must finish before held workspace writes, SQL writers and unrelated reads
release. Test distinct-scope parallel work, same-scope ordering and all lock
order permutations; crash/failure/cancellation publication recovery; every
affected view's RYOW; cold/warm read behavior; physical-repo exclusions.
Run audited bridge unit/integration/contract tests, Clippy and fmt under
`nice -n 10`, judged by exit status. Full suites are allowed under this
assignment's constraints only after their fixtures avoid forbidden paths,
environment and executable use. Gitleaks before push; applicable Semgrep and
`git diff --check` before ready. No live validation is needed for acceptance.

## 5. Principal risks and unresolved facts

1. **State split loses a hidden invariant.** Broad mutations currently rely
   on one lock for registry membership, delivery authority, config rewrites,
   attention and cleanup. Shared Git repositories also cross logical scope
   boundaries. Mitigate with complete declared ownership, canonical physical
   locks, existing generation/receipt guards, and failure/interleaving tests.
   The inventory below marks transitive closures not yet proven; migrating
   one requires closing that audit, not treating an unlisted field as safe.
2. **Commit, publication and history generations diverge.** Off-lock SQL can
   expose committed rows before memory catches up, stale refreshes can undo
   new views, and reset can mix histories. Mitigate with one transactional
   authority per invariant, owned settlement jobs, versioned merged
   publication and ack only after affected views reflect the write. Memory
   usage and publication cost for the actual installation are unmeasured.
3. **The queue merely moves elsewhere.** Read pools, admission, shared
   blocking threads, config persistence, push facts, checkpoints or the
   SQLite writer can become the next bottleneck. Mitigate with end-to-end
   phase timing, independent bounded executors, immediate overload refusal,
   and tests that saturate every competing slot. Pool sizes, real verb
   frequency, SQLite share of latency and useful latency budgets remain
   unestablished until the private baseline runs.

## 6. Verification of this planning change

Only documentation is changed. The proposed tests above have not been
implemented or claimed to pass.

Executed from `bridge/`, with real exit status **0** for each:

- `nice -n 10 cargo test --locked --offline --lib timing::tests -- --nocapture`
  — **17 passed**, 0 failed. These exercise measured queue/lock phases and
  stats/slow-log reporting on synthetic locks, not real application workloads.
- `nice -n 10 cargo test --locked --offline --lib carrier::dispatch::dispatcher_tests -- --nocapture`
  — **25 passed**, 0 failed. Covers queue bounds, priorities, ordering,
  folding, cancellation, caller wires and session endings.

One observed synthetic slow-frame line in the dispatcher run was
`term.ack total=301.0ms queued=301.0ms lock_wait=0.0ms held=0.0ms`:
this test intentionally holds terminal input and demonstrates queue delay
without app-lock delay. Another synthetic handler logged
`board.list total=203.5ms queued=0.3ms lock_wait=0.0ms held=0.0ms`;
this is a gated fake handler, **not board.list application performance**.
The tests establish instrumentation/scheduler behavior; they cannot identify
the dominant production verbs or split SQLite cost.

The existing ignored PTY concurrency test sets a prohibited environment
variable. Existing AppState fixtures can resolve/create the default state
home and consult shell environment even in QA mode. Those tests were not
run. Step 1 explicitly fixes fixture construction before broader tests.
No new performance baseline was fabricated from the passing test counts.

Document verification checks exact registry/table coverage, referenced source
paths/functions, and whitespace. Scan results are recorded at handoff.

## 7. Verb classification and AppState footprint

Registered inventory: **138 exact names, 12 families** in `bridge/src/api/v1/mod.rs::families/methods`; table below contains every name once. Method family wrappers call the AppState functions indexed in the appendix. Not an implementation or a claim of full transitive purity.

### 7.1 Classification rules and field legend

- “Snapshot” denotes an in-memory read (M). In the target it is an immutable published value obtained without an app/domain lock. SQL reads are S; writes (including hidden maintenance) are W. I/O denotes a filesystem/Git/CLI read, an explicit additional category because calling these SQLite-only or memory-only would be false. “SQL + snapshot” must not be advertised as SQLite-only. No v1 read traced here is proven to require *only* SQLite with no current AppState identity validation/data; `tasks.waiting_on.blocks`, `thread.operation` payload and task/review stored payloads are candidates after explicit context extraction.
- Write classifications include database/file writes, maintenance, cache/registry mutation, and queue/subscription changes; Rust `&self` and a read-like verb name do not prove read-only behavior.
- Scope column is a proposed semantic lock domain, not an existing code fact. Existing code uses the app mutex, internal service/store locks, source/path locks and deferred reservations. Project/workspace effects on shared device config require a separate short config publication domain. Global maintenance cannot remain in read handlers if parallel reads are intended.
- Legend names exact `AppState` fields, **not an assertion that every verb referencing a group touches every field or that its complete transitive closure is certified**. Per-handler direct field evidence is appended. Helper effects explicitly listed in rows were inspected; unlisted downstream persistence/runtime details remain audit work.
- **P**: `projects`; project update/delete also have `source_moves`, `project_deletion_in_progress`, `active_deferred_filesystem_jobs` where named.
- **W**: `workspaces`, `workspace_lifecycle`, plus `reclaim_reserved`/`reclaim_policy`/`reclaim_stop` where reclaim guards participate.
- **E**: `plans`, `runs`, their nested agent/conversation state; canonical binding uses project/workspace ownership. Session projections additionally use `session_summaries`, `session_seen`, `session_registry` as applicable.
- **B**: `board` (attention, archive and caches), `vanished_run_sweep_in_flight`, deferred cache/sweep scheduling; `pending_rows`, `usage_limits` are board inputs.
- **C**: `projects_dir`, `default_harness`, `project_agent`, `role_models`, `agent_modes`, `isolation`, `watch_agent_filed_tasks`, `compact_above_tokens`, `compaction_policies`, `config_path`, `reclaim_settings`, `reclaim_policy`; only each setting's actual subset, not whole config copying.
- **T**: `store` (internally mutable durable state); task live identity joins additionally P/W/E; task list joins `user_session`.
- **R**: runtime collaborators `session_registry`, `delivery_queue`, `operation_ledger`, `router_sessions`, `dispatched_task`, `task_completion_authority`, `spent_task_completion_operations`, `pending_task_completion_authority`, `task_delivery_in_flight`, `reminded_holdings`, `agent_failures`, `compactions`, `resetting_conversations`, `streams` as applicable; full member-by-member closure for each write is **not established** by this scoped review.
- **G**: resolved Git/filesystem inputs from `projects`, `runs`, `workspaces`, external checkout/cache state in `board`; `deferred_work` for deferred jobs, `self_handle` for source validation, `reclaim_reserved` for writes. Reading workspace IDs can presently adopt legacy records.
- Every v1 call also runs `api/v1/mod.rs::dispatch -> expect_deferred_result`, which sets the shared `deferred_result_check` for an existing deferred job. Together with single-slot `deferred_work`, this must become a per-request outcome before concurrent dispatch.

### 7.2 Exhaustive v1 table

| Verbs | Current behavior / proposed read form | Proposed write scope | Known field groups | Evidence and exceptions |
| --- | --- | --- | --- | --- |
| `board.list` | W now; target M published board | device maintenance, then no read lock | B, E, P, W, T | board/views::board_list -> sweep_vanished_runs, run_stat/cache refresh, external_worktrees_json, workspace_summaries_json, work_items, watched_task_rows. Persists attention on newly observed/crossed dismissal rows; can recut project scratch directories. Watcher/cache/sweep apply phases also mutate later. |
| `archived.list` | snapshot + filesystem read | none | B, E, P, W | board/archive::archived_list reads retained plans/runs, board archive and registered projects; WorkspaceRegistry::finished_records (workspace.rs:907) scans/reads files in registry .legacy directory, so it is not memory-only. |
| `project.list` | W now; M published project rows after maintenance/probe extraction | device registry maintenance, then none | P, E, C, deferred_work | projects/list::defer_project_list calls revive_parked_projects first (projects/parked.rs: restores projects/rebinds retained entities/emits changes), then owned ProjectListRow snapshots; render probes repositories/volumes off lock already. |
| `project.create` | write | device registry creation + new project | P, C, deferred_work | projects/requests::project_create; allocates project/source directories or clone work and persists config. Scope assignment needs device allocation followed by new project reservation. |
| `project.delete` | write | device registry + project + all affected workspaces | P, W, E, R, C, T | projects/deletion::project_delete currently sets global project_deletion_in_progress; rejects while active_deferred_filesystem_jobs nonzero; retires conversations and resources. Multi-resource destructive exception, not a workspace-only write. |
| `project.init_git`, `project.set_remote`, `project.add_source`, `project.remove_source`, `project.set_isolation` | write | project; registry/config publication is device metadata | P, C, deferred_work | projects/requests functions same as verb with underscores; mutate source/registration/config, filesystem/Git where applicable. Full transitive field closures not certified. |
| `project.update_source` | write | project + every workspace checkout affected by changed source | P, W, source_moves, C, deferred_work | projects/source_update::project_update_source reserves source moves; remote update can rewrite origins in multiple workspace checkouts. Preserve source identity/path guards and source_sync locks. |
| `project.sync_source` | write (service request) | project | P, source_sync_requested, source_sync_nudge | projects/base_sync::project_sync_source -> request_source_sync enqueues captured source and notifies service; later service applies observed status. |
| `project.ensure_conversation` | write | project | P, E, R, store, state_root | projects/conversation::project_ensure_conversation may create project scratch, run/conversation and pin. Already-present case is idempotent; classify entire verb write. |
| `project.set_compaction`, `workspace.set_compaction` | write | project / workspace, plus device config serialization | C, P, W, changes | config/compaction_wire::{set_project_compaction,set_workspace_compaction} -> patch_compaction_thresholds persists shared config; workspace result invokes workspace_get and its hidden maintenance. Do not conflate owner scope with config-file write scope. |
| `capture.get` | snapshot read | none | captures | captures/mod::capture_get only reads captures map and renders row. |
| `capture.create`, `capture.answer`, `capture.cancel`, `capture.reroute` | write | device capture/router; reroute adds selected project/workspace | captures, router_sessions, store; E/R/P downstream | captures/mod corresponding functions persist capture, start/abandon router; reroute invokes route_to_branch. Cannot choose all final project/workspace scopes from params when routing is asynchronous; split capture mutation from routed dispatch. |
| `settings.get` | snapshot + external probe now; target M cached observation | none | C, P | config/settings::settings_get calls account_availability -> first project worktrees availability; move volume probe off app lock or cached service observation. |
| `settings.set` | write | device config | C, P, changes, reclaim_nudge | config/settings::settings_set validates prospective config, creates/canonicalizes projects_dir, persists before updating memory, nudges reclaim and emits compaction changes. Preserve disk-failure rollback. |
| `models.list` | snapshot read | none | default_harness, agent_modes, role_models, cli_readings | config/settings::models_list uses cached CLI readings/catalogs; no CLI invoked here. |
| `user.present` | write | device user session | user_session, store, changes | session_summaries::user_present -> note_user_activity persists and emits device-wide task/session invalidation. |
| `changes.subscribe`, `changes.unsubscribe` | write (session subscription) | session/device subscription registry; watcher paths may span projects | changes, watchers, deferred_work | api/v1/changes::{changes_subscribe,changes_unsubscribe}; subscribe checks source scope; mutable ChangeBus subscription state, then deferred watch reconciliation. SessionSender comes from with_session thread-local; must become explicit request context before async/split dispatch. |
| `git.log`, `git.show`, `git.status`, `git.diff`, `git.changeset_diff`, `git.refs`, `git.unpushed`, `worktree.diff`, `run.diff`, `task.stage_diff` | snapshot + off-lock Git read; scope can write today | none after resolver maintenance extraction | G | git/mod corresponding methods (git.changeset_diff -> changeset_diff), runs/review::run_diff, tasks/views::task_stage_diff. Existing deferred Git/read work is off lock, but resolver can adopt legacy workspaces; external-worktree/cache resolution needs audit. Git/status filesystem effects not fully audited; never declare these SQLite reads. |
| `git.stage`, `git.discard`, `git.commit`, `git.fetch`, `git.pull`, `git.push`, `git.stash`, `git.stash_pop`, `git.merge_abort`, `git.checkout_ref` | write | resolved workspace OR project source; common repository/path lock when aliases share Git | G, B, W, changes | git/mod corresponding functions -> defer_git(invalidates=true); apply_git may reopen workspace and invalidates caches/attention. Scope is server-resolved, not raw optional request project_id. Existing serialized ScopedGitCall/source validation must survive. |
| `fs.list` | external filesystem read | none | no direct AppState field | app/fs::fs_list canonicalizes/browses host directories. Not SQLite or a memory-only read; off-lock I/O job with no app snapshot needed. |
| `fs.mkdir` | write | device/host directory namespace | reclaim_reserved through refusal helper | app/fs::fs_mkdir creates arbitrary user-selected child outside project/workspace shape. Requires explicit host-path exception, not invented workspace ownership. |
| `fs.tree`, `fs.read` | snapshot + filesystem read; scope may write today | none after resolver maintenance extraction | G | app/fs::{fs_tree,fs_read}, FileScope::resolve_root -> resolve_workspace_source / project-source / TermScope. Workspace resolution can adopt legacy workspaces; all file I/O currently executes under app handler lock. |
| `fs.write`, `fs.createDirectory`, `fs.createFile` | write | resolved workspace/project source or legacy owner | G, W, B, changes | app/fs::{fs_write,fs_create_directory,fs_create_file}: reservation guard, fenced file change, reopen workspace, invalidate_file_scope; retained descriptors/revision check needed across off-lock work. |
| `fs.uploadBegin`, `fs.uploadChunk`, `fs.uploadFinish`, `fs.uploadAbort` | write (session upload lifecycle) | upload owner session + resolved workspace/project/path | uploads, G; W/B/changes at finish | app/fs/uploads corresponding snake_case functions retain session ownership, staged file and counters; chunk/finish/abort params only upload_id so recover scope from admitted Upload, never re-guess. Scoped gate must not remain held over all chunks indefinitely. |
| `github.repos` | snapshot + external CLI/network read | none | github, deferred_work | app/github::defer_github_repos clones CLI handle into deferred external request; not SQLite. Existing off-lock gh deadline remains. |
| `harnesses.list`, `bridge.update_status` | snapshot service read | none | harness_inventory / updates | api/v1/harnesses::list -> inventory.snapshot; api/v1/updates::update_status -> update service status. Clone service handle without app lock if held in request context. |
| `harnesses.refresh`, `bridge.check_update` | write (service request) | device/service | harness_inventory / updates | api/v1/harnesses::refresh -> request_refresh; updates::check_update -> request_check. They schedule observations, so not pure reads despite small synchronous result. |
| `bridge.install_update` | write (service request) | device admission/update | updates, R | api/v1/updates::install_update reads update_has_working_agents (delivery_queue, router_sessions, session_registry, plans, runs) then request_install. Preserve admission behavior and coherent busy snapshot. |
| `task.get` | snapshot + SQL history read | none | plans, E, T | tasks/views::plan_get obtains detail_thread_value (may read stored history) then plan_view. Retired singular task flow differs from tracker tasks.*. |
| `task.stages` | snapshot read | none | plans, runs | tasks/views::task_stages composes plan stage/doc comments and current implementation progress from retained state. |
| `task.doc`, `task.stage_doc` | snapshot + filesystem document read | none | plans, store | tasks/documents::{plan_doc,plan_stage_doc}; immutable plan identity/path resolved from plans then Store::read_plan_doc (store/entities.rs:1030), which is std::fs::read_to_string under plan_docs_dir, not SQLite. |
| `entity.seen`, `entity.mute`, `entity.dismiss` | write | canonical conversation/project/workspace owner; branch dismissal project | B, E, store, user_session, changes | board/attention corresponding functions; persist marks/mutes/dismissals and propagate user activity/notifications. Legacy branch-only dismiss has project+branch instead of entity. |
| `run.request_changes`, `run.git_action`, `run.message`, `run.abandon`, `run.adopt`, `branch.dispatch`, `branch.finish` | write | resolved project/workspace; dispatch/adopt/finish may cross both | E, P, W, B, R, store, deferred_work | runs/review::{run_request_changes,run_git_action}; runs/lifecycle::{run_message,run_abandon,run_adopt}; worktrees/dispatch::branch_dispatch; workspaces/mod::workspace_finish_legacy. Preserve deferred reserve/work/apply and canonical owner aliases. Full downstream write closure remains implementation audit. |
| `push.registerKey`, `push.revokeKey` | write | device notification key namespace | store | app/push_keys::{register_push_key,revoke_push_key} only validate then upsert/delete Store key; good first DB-worker write conversion. |
| `tasks.review.get` | snapshot + SQL + filesystem/Git projection read | none | store, projects, workspaces, reclaim_reserved, deferred_work | tracker/reviews::review_get validates task/project, loads review, computes review_action_sources with reservation/current-source facts, defers actions::with_destinations + projection::result. Not SQLite-only today. |
| `tasks.review.diff` | snapshot + SQL + Git read | none | store, projects, deferred_work | tracker/reviews::review_diff loads task/review, clones selected snapshot directory, defers reviews::read::read. Immutable revision identity must remain attached through I/O. |
| `tasks.review.open`, `tasks.review.push`, `tasks.review.update`, `tasks.review.merge`, `tasks.review.close`, `tasks.review.reopen`, `tasks.review.refresh`, `tasks.review.snapshot`, `tasks.review.act`, `tasks.review.complete` | write | project task + review workspace + any destination/source checkouts | store, P, W, E, R, changes, deferred_work | tracker/reviews/{opening,publication,merge,lifecycle}.rs and tracker/reviews.rs::review_{snapshot,act,complete}. refresh/snapshot are durable writes (PR reconciliation), not read names. Merge and act can write source/base destinations, so source/path gates plus multi-scope ordering are required. |
| `tasks.list`, `tasks.get` | write today (backfill); target snapshot + SQL read after migration | project migration; then none for reads | T, E, P, W, user_session | tracker/mod::{tasks_list,listed_rows,tasks_get}: backfill_task_identities and backfill_done_at call Store::backfill_tracker_task; live identity projection consults in-memory owners/workspaces. list also includes user_session and tracker_list_key. These cannot be classified SQLite-only merely because task rows live in SQLite. |
| `tasks.waiting_on.blocks` | snapshot scope + SQL read | none | store, projects | tracker/waiting_on_api::tasks_waiting_on_blocks -> tracker_task (project existence/canonical lookup), Store::waiting_on_blocks. Strong SQL-only candidate once project identity mapping is immutable request context. |
| `tasks.columns` | snapshot read | none | projects | tracker/mod::tasks_columns only validates project then emits fixed columns_json; does not read SQLite. |
| `tasks.create`, `tasks.assign` | write | project task + target conversation/workspace; may allocate workspace | T, P, W, E, R, deferred_work | tracker/mod::tasks_create; tracker/dispatch::tasks_assign. Dispatch can reserve/provision checkout and enqueue durable delivery. Final scope may depend on resolved assignment target; resolve/revalidate before acquiring set. |
| `tasks.update`, `tasks.comment`, `tasks.waiting_on.add`, `tasks.waiting_on.remove`, `tasks.close`, `tasks.reopen`, `tasks.watch`, `tasks.unwatch`, `tasks.read_through` | write | task project; delivery/linked-owner followups may additionally require workspace | T, E, P, W, R, changes | tracker/mod::{tasks_update,tasks_comment,tasks_close,tasks_reopen}; waiting_on_api::tasks_waiting_on_edit; tracking::{tasks_watch,tasks_unwatch,tasks_read_through}. Changes include task record/timeline/dependencies, identity capture, user session, notification/delivery. Helper mutation closures not fully audited: conservatively retain coordinator gate until split. |
| `tasks.attach` | write | task project + shared attachment store namespace | store, projects, state_root | tracker/attachments::tasks_attach validates task/project then writes shared local attachment bytes; file names/ownership must remain fenced. |
| `tasks.attachment` | snapshot scope + SQL + filesystem read | none | store, projects, state_root | tracker/attachments::tasks_attachment validates task through tracker_task, resolves shared attachment store and reads bounded content. Pure external bytes phase should be off app lock. |
| `thread.page`, `thread.activity` | snapshot + optional SQL history read | none | E, store | conversations/read::{thread_page,thread_activity}; resident-tail pages are memory; old/lean pages call Store history. Capture canonical binding, conversation id, metadata and sequence bounds together; do not copy whole AppState or entire history. |
| `thread.revision` | snapshot read | none | E | conversations/read::thread_revision reads canonical conversation retained revisions/snapshot bytes; unknown revision or discarded snapshot remains same error. |
| `thread.operation` | snapshot binding + SQL read (memory fallback) | none | E, store, operation_ledger | conversations/post::thread_operation validates entity/conversation, reads Store::operation or operation_ledger.cached without store. Candidate SQL payload read after owned binding snapshot; preserve receipt owner/agent checks. |
| `thread.attachment` | snapshot binding + filesystem read | none | E, state_root and attachment-root resolver fields | conversations/attachments::thread_attachment validates binding then resolves attachment path and reads file. Full resolver field closure not certified; preserve canonical containment and bounded range behavior. |
| `thread.post`, `thread.attach` | write | canonical conversation owner project/workspace; durable delivery may touch task project | E, R, store; attachment paths | conversations/post::thread_post, attachments::thread_attach. post is operation-ledger acceptance plus thread/delivery; attach writes file and may Store ownership. Canonical binding/generation/operation id must cross off-lock worker intact. |
| `agent.list` | snapshot read | none | E, P, W, session_registry, C, usage_limits | runtime/agents/endpoints::agent_list -> listed_agents / agent_digest; agent_digest includes runtime/settings observations. No visible mutation from outer &mut signature; full digest helper closure must be confirmed before minimizing fields. |
| `agent.add`, `agent.choose`, `agent.remove`, `conversation.watch`, `conversation.unwatch`, `conversation.reset`, `conversation.settings` | write | canonical project/workspace conversation; removal can preserve task identities across project | E, R, B, P, W, store, C, changes | runtime/agents/endpoints::{agent_add,agent_choose,agent_remove}; runtime/agents/records::{set_conversation_watched,set_conversation_context_limit}; conversations/reset::conversation_reset. agent.remove preserves historical project task identities before roster disappears. Single workspace-only lock would miss those tasks. Some aliases point to another canonical conversation owner. |
| `workspace.list`, `workspace.get` | W now; target M published workspace views | global workspace maintenance currently; target none | W, P, E, C, T | workspaces/mod::{workspace_list,workspace_get} always adopt legacy workspaces then refresh_local_capabilities (git2 read + manifest persistence) and refresh_finished_local (git subprocesses + reopen/persist). Detail calls run_get; both append review summaries from Store. Move maintenance to service and batch DB reads, never simply change &mut to &self. |
| `workspace.git_init_options` | snapshot + external probe read; scope may write today | none after legacy adoption extraction | projects, workspaces | workspaces/git_initialization::workspace_git_init_options -> resolve_workspace_directory, git_init_option; probes both workspace and original source locations. Resolver can adopt globally when workspace absent. |
| `workspace.create`, `workspace.retry` | write | project allocation + target workspace | W, P, C, deferred_work | workspaces/mod::{workspace_create,workspace_retry}; provisioning reserves filesystem state and source/base sync then applies. Multi-source inputs need project/source coordination; all slow work remains off app lock. |
| `workspace.ensure_conversation` | write | workspace + project binding | workspaces, projects, runs, default_harness, store | workspaces/mod::workspace_ensure_conversation creates run and projects.bind_entity then finish_run_mutation; rollback removes run/forgets it on failure. |
| `workspace.init_git` | write | workspace and/or project source depending target; both = multi-scope | W, P, deferred_work | workspaces/git_initialization::workspace_init_git accepts target workspace/source/both; cannot serialize only by workspace id when writing original source. |
| `workspace.add_directory`, `workspace.remove_directory`, `workspace.rename`, `workspace.set_locked` | write | workspace; add source also project/source coordination | W, P, B, deferred_work | workspaces/directories corresponding methods; workspaces/mod::workspace_rename; workspaces/lock::workspace_set_locked. rename/set_locked use active_deferred_filesystem_jobs guards. Output detail may invoke global read maintenance today. |
| `workspace.finish`, `workspace.delete`, `workspace.reclaim` | write | workspace + project-linked task/conversation cleanup + source/path cleanup | W, P, E, R, T, B, reclaim_reserved, deferred_work | workspaces/mod::workspace_finish; deletion::workspace_delete; reclaim/explicit::workspace_reclaim. Reservations, source hooks, linked task closure/identity preservation, retirement and filesystem phases mean multi-scope. Preserve guards through failure/cancellation and cleanup. |
| `workspace.measure_sizes` | write (queue request) | selected workspace set or device-wide queue admission | workspaces, workspace_lifecycle, size_requests | workspaces/sizes::workspace_measure_sizes reads measurable workspaces/freshness and enqueues SizeRequests; size walker later updates workspace_lifecycle. Do not mark pure read from &self signature. |

### 7.3 Per-verb handler/function evidence and direct fields

The following table is deliberately narrower than a full effects analysis: it gives the typed v1 function and **direct `self.field` references inside the immediately called AppState function**, excluding test-only fields. `none directly` means that helpers do the work, not that the verb is pure. This textual inventory was checked against the 138 registrations; field reads and writes are not conflated into a claimed write set. Direct function extraction is not a Rust semantic analyzer, so use the file/function as authority for implementation review.

| Verb | Typed v1 wrapper | Immediate AppState target (source line) | Direct AppState fields |
| --- | --- | --- | --- |
| `project.set_compaction` | `bridge/src/api/v1/board.rs::set_compaction` | `bridge/src/app/config/compaction_wire.rs:69::set_project_compaction` | `projects` |
| `board.list` | `bridge/src/api/v1/board.rs::board_list` | `bridge/src/app/board/views.rs:463::board_list` | `projects`, `runs` |
| `archived.list` | `bridge/src/api/v1/board.rs::archived_list` | `bridge/src/app/board/archive.rs:18::archived_list` | `board`, `plans`, `projects`, `runs`, `workspaces` |
| `project.delete` | `bridge/src/api/v1/board.rs::project_delete` | `bridge/src/app/projects/deletion.rs:67::project_delete` | `active_deferred_filesystem_jobs`, `default_harness`, `deferred_work`, `delivery_queue`, `isolation`, `project_deletion_in_progress`, `projects`, `projects_dir`, `runs`, `session_registry`, `workspaces` |
| `project.list` | `bridge/src/api/v1/board.rs::project_list` | `bridge/src/app/projects/list.rs:62::defer_project_list` | `deferred_work` |
| `project.create` | `bridge/src/api/v1/board.rs::project_create` | `bridge/src/app/projects/requests.rs:233::project_create` | `projects_dir` |
| `project.init_git` | `bridge/src/api/v1/board.rs::project_init_git` | `bridge/src/app/projects/requests.rs:180::project_init_git` | `projects` |
| `project.set_remote` | `bridge/src/api/v1/board.rs::project_set_remote` | `bridge/src/app/projects/requests.rs:517::project_set_remote` | `projects` |
| `project.add_source` | `bridge/src/api/v1/board.rs::project_add_source` | `bridge/src/app/projects/requests.rs:431::project_add_source` | `projects_dir` |
| `project.remove_source` | `bridge/src/api/v1/board.rs::project_remove_source` | `bridge/src/app/projects/requests.rs:473::project_remove_source` | `projects` |
| `project.update_source` | `bridge/src/api/v1/board.rs::project_update_source` | `bridge/src/app/projects/source_update.rs:52::project_update_source` | `source_moves` |
| `project.sync_source` | `bridge/src/api/v1/board.rs::project_sync_source` | `bridge/src/app/projects/base_sync.rs:651::project_sync_source` | none directly; see helper effects |
| `project.set_isolation` | `bridge/src/api/v1/board.rs::project_set_isolation` | `bridge/src/app/projects/requests.rs:543::project_set_isolation` | `projects` |
| `project.ensure_conversation` | `bridge/src/api/v1/board.rs::project_ensure_conversation` | `bridge/src/app/projects/conversation.rs:91::project_ensure_conversation` | `projects`, `runs`, `state_root` |
| `capture.create` | `bridge/src/api/v1/board.rs::capture_create` | `bridge/src/app/captures/mod.rs:62::capture_create` | `captures` |
| `capture.get` | `bridge/src/api/v1/board.rs::capture_get` | `bridge/src/app/captures/mod.rs:215::capture_get` | `captures` |
| `capture.answer` | `bridge/src/api/v1/board.rs::capture_answer` | `bridge/src/app/captures/mod.rs:93::capture_answer` | `captures` |
| `capture.reroute` | `bridge/src/api/v1/board.rs::capture_reroute` | `bridge/src/app/captures/mod.rs:158::capture_reroute` | `captures` |
| `capture.cancel` | `bridge/src/api/v1/board.rs::capture_cancel` | `bridge/src/app/captures/mod.rs:128::capture_cancel` | `captures` |
| `settings.get` | `bridge/src/api/v1/board.rs::settings_get` | `bridge/src/app/config/settings.rs:13::settings_get` | `agent_modes`, `compact_above_tokens`, `compaction_policies`, `default_harness`, `isolation`, `project_agent`, `projects_dir`, `reclaim_policy`, `role_models`, `watch_agent_filed_tasks` |
| `settings.set` | `bridge/src/api/v1/board.rs::settings_set` | `bridge/src/app/config/settings.rs:116::settings_set` | `agent_modes`, `compact_above_tokens`, `compaction_policies`, `default_harness`, `isolation`, `project_agent`, `projects_dir`, `reclaim_settings`, `role_models`, `watch_agent_filed_tasks` |
| `models.list` | `bridge/src/api/v1/board.rs::models_list` | `bridge/src/app/config/settings.rs:41::models_list` | `agent_modes`, `cli_readings`, `default_harness`, `role_models` |
| `user.present` | `bridge/src/api/v1/board.rs::user_present` | `bridge/src/app/session_summaries.rs:156::user_present` | none directly; see helper effects |
| `changes.subscribe` | `bridge/src/api/v1/changes.rs::changes_subscribe` | `bridge/src/app/projects/source_scope.rs:104::check_source_subscription`; `changes` (accessor or follow-up lookup); `bridge/src/app/runtime/deferred.rs:808::defer_watch` | `changes`, `deferred_work`, `watchers` |
| `changes.unsubscribe` | `bridge/src/api/v1/changes.rs::changes_unsubscribe` | `changes` (accessor or follow-up lookup); `bridge/src/app/runtime/deferred.rs:808::defer_watch` | `changes`, `deferred_work`, `watchers` |
| `git.log` | `bridge/src/api/v1/git.rs::git_log` | `bridge/src/app/git/mod.rs:619::git_log` | none directly; see helper effects |
| `git.show` | `bridge/src/api/v1/git.rs::git_show` | `bridge/src/app/git/mod.rs:642::git_show` | none directly; see helper effects |
| `git.status` | `bridge/src/api/v1/git.rs::git_status` | `bridge/src/app/git/mod.rs:668::git_status` | none directly; see helper effects |
| `git.diff` | `bridge/src/api/v1/git.rs::git_diff` | `bridge/src/app/git/mod.rs:689::git_diff` | none directly; see helper effects |
| `git.changeset_diff` | `bridge/src/api/v1/git.rs::git_changeset_diff` | `bridge/src/app/git/mod.rs:275::changeset_diff` | none directly; see helper effects |
| `git.stage` | `bridge/src/api/v1/git.rs::git_stage` | `bridge/src/app/git/mod.rs:701::git_stage` | none directly; see helper effects |
| `git.discard` | `bridge/src/api/v1/git.rs::git_discard` | `bridge/src/app/git/mod.rs:800::git_discard` | none directly; see helper effects |
| `git.commit` | `bridge/src/api/v1/git.rs::git_commit` | `bridge/src/app/git/mod.rs:714::git_commit` | none directly; see helper effects |
| `git.fetch` | `bridge/src/api/v1/git.rs::git_fetch` | `bridge/src/app/git/mod.rs:752::git_fetch` | none directly; see helper effects |
| `git.pull` | `bridge/src/api/v1/git.rs::git_pull` | `bridge/src/app/git/mod.rs:761::git_pull` | none directly; see helper effects |
| `git.push` | `bridge/src/api/v1/git.rs::git_push` | `bridge/src/app/git/mod.rs:771::git_push` | none directly; see helper effects |
| `git.stash` | `bridge/src/api/v1/git.rs::git_stash` | `bridge/src/app/git/mod.rs:783::git_stash` | none directly; see helper effects |
| `git.stash_pop` | `bridge/src/api/v1/git.rs::git_stash_pop` | `bridge/src/app/git/mod.rs:791::git_stash_pop` | none directly; see helper effects |
| `git.merge_abort` | `bridge/src/api/v1/git.rs::git_merge_abort` | `bridge/src/app/git/mod.rs:810::git_merge_abort` | none directly; see helper effects |
| `git.refs` | `bridge/src/api/v1/git.rs::git_refs` | `bridge/src/app/git/mod.rs:599::git_refs` | none directly; see helper effects |
| `git.checkout_ref` | `bridge/src/api/v1/git.rs::git_checkout_ref` | `bridge/src/app/git/mod.rs:608::git_checkout_ref` | none directly; see helper effects |
| `git.unpushed` | `bridge/src/api/v1/git.rs::git_unpushed` | `bridge/src/app/git/mod.rs:676::git_unpushed` | none directly; see helper effects |
| `fs.list` | `bridge/src/api/v1/git.rs::fs_list` | `bridge/src/app/fs.rs:232::fs_list` | none directly; see helper effects |
| `fs.mkdir` | `bridge/src/api/v1/git.rs::fs_mkdir` | `bridge/src/app/fs.rs:274::fs_mkdir` | none directly; see helper effects |
| `fs.tree` | `bridge/src/api/v1/git.rs::fs_tree` | `bridge/src/app/fs.rs:304::fs_tree` | none directly; see helper effects |
| `fs.read` | `bridge/src/api/v1/git.rs::fs_read` | `bridge/src/app/fs.rs:317::fs_read` | none directly; see helper effects |
| `fs.write` | `bridge/src/api/v1/git.rs::fs_write` | `bridge/src/app/fs.rs:367::fs_write` | none directly; see helper effects |
| `fs.createDirectory` | `bridge/src/api/v1/git.rs::fs_create_directory` | `bridge/src/app/fs.rs:175::fs_create_directory` | none directly; see helper effects |
| `fs.createFile` | `bridge/src/api/v1/git.rs::fs_create_file` | `bridge/src/app/fs.rs:192::fs_create_file` | none directly; see helper effects |
| `fs.uploadBegin` | `bridge/src/api/v1/git.rs::fs_upload_begin` | `bridge/src/app/fs/uploads.rs:209::fs_upload_begin` | `uploads` |
| `fs.uploadChunk` | `bridge/src/api/v1/git.rs::fs_upload_chunk` | `bridge/src/app/fs/uploads.rs:169::fs_upload_chunk` | `uploads` |
| `fs.uploadFinish` | `bridge/src/api/v1/git.rs::fs_upload_finish` | `bridge/src/app/fs/uploads.rs:109::fs_upload_finish` | `uploads` |
| `fs.uploadAbort` | `bridge/src/api/v1/git.rs::fs_upload_abort` | `bridge/src/app/fs/uploads.rs:99::fs_upload_abort` | `uploads` |
| `worktree.diff` | `bridge/src/api/v1/git.rs::worktree_diff` | `bridge/src/app/git/mod.rs:247::worktree_diff` | none directly; see helper effects |
| `run.diff` | `bridge/src/api/v1/git.rs::run_diff` | `bridge/src/app/runs/review.rs:52::run_diff` | none directly; see helper effects |
| `task.stage_diff` | `bridge/src/api/v1/git.rs::task_stage_diff` | `bridge/src/app/tasks/views.rs:127::task_stage_diff` | `board`, `runs` |
| `github.repos` | `bridge/src/api/v1/github.rs::repos` | `bridge/src/app/github.rs:17::defer_github_repos` | `deferred_work`, `github` |
| `harnesses.list` | `bridge/src/api/v1/harnesses.rs::list` | `bridge/src/app/harness_inventory.rs:23::harness_inventory` | `harness_inventory` |
| `harnesses.refresh` | `bridge/src/api/v1/harnesses.rs::refresh` | `bridge/src/app/harness_inventory.rs:23::harness_inventory` | `harness_inventory` |
| `task.get` | `bridge/src/api/v1/lifecycle.rs::task_get` | `bridge/src/app/tasks/views.rs:107::plan_get` | `plans` |
| `task.doc` | `bridge/src/api/v1/lifecycle.rs::task_doc` | `bridge/src/app/tasks/documents.rs:36::plan_doc` | `plans` |
| `task.stages` | `bridge/src/api/v1/lifecycle.rs::task_stages` | `bridge/src/app/tasks/views.rs:166::task_stages` | `plans` |
| `task.stage_doc` | `bridge/src/api/v1/lifecycle.rs::task_stage_doc` | `bridge/src/app/tasks/documents.rs:51::plan_stage_doc` | `plans` |
| `entity.seen` | `bridge/src/api/v1/lifecycle.rs::entity_seen` | `bridge/src/app/board/attention.rs:738::entity_seen` | `changes` |
| `entity.mute` | `bridge/src/api/v1/lifecycle.rs::entity_mute` | `bridge/src/app/board/attention.rs:777::entity_mute` | `board` |
| `entity.dismiss` | `bridge/src/api/v1/lifecycle.rs::entity_dismiss` | `bridge/src/app/board/attention.rs:812::entity_dismiss` | `board` |
| `run.request_changes` | `bridge/src/api/v1/lifecycle.rs::run_request_changes` | `bridge/src/app/runs/review.rs:136::run_request_changes` | `delivery_queue`, `runs` |
| `run.git_action` | `bridge/src/api/v1/lifecycle.rs::run_git_action` | `bridge/src/app/runs/review.rs:231::run_git_action` | `plans`, `runs` |
| `run.message` | `bridge/src/api/v1/lifecycle.rs::run_message` | `bridge/src/app/runs/lifecycle.rs:240::run_message` | `delivery_queue`, `qa_agent`, `runs` |
| `run.abandon` | `bridge/src/api/v1/lifecycle.rs::run_abandon` | `bridge/src/app/runs/lifecycle.rs:283::run_abandon` | `runs` |
| `run.adopt` | `bridge/src/api/v1/lifecycle.rs::run_adopt` | `bridge/src/app/runs/lifecycle.rs:492::run_adopt` | `default_harness` |
| `branch.dispatch` | `bridge/src/api/v1/lifecycle.rs::branch_dispatch` | `bridge/src/app/worktrees/dispatch.rs:70::branch_dispatch` | none directly; see helper effects |
| `branch.finish` | `bridge/src/api/v1/lifecycle.rs::branch_finish` | `bridge/src/app/workspaces/mod.rs:924::workspace_finish_legacy` | `workspaces` |
| `push.registerKey` | `bridge/src/api/v1/push.rs::register_key` | `bridge/src/app/push_keys.rs:40::register_push_key` | none directly; see helper effects |
| `push.revokeKey` | `bridge/src/api/v1/push.rs::revoke_key` | `bridge/src/app/push_keys.rs:54::revoke_push_key` | none directly; see helper effects |
| `tasks.review.open` | `bridge/src/api/v1/reviews.rs::open` | `bridge/src/app/tracker/reviews/opening.rs:43::review_open` | `workspaces` |
| `tasks.review.push` | `bridge/src/api/v1/reviews.rs::push` | `bridge/src/app/tracker/reviews/publication.rs:91::review_push` | `self_handle`, `workspaces` |
| `tasks.review.update` | `bridge/src/api/v1/reviews.rs::update` | `bridge/src/app/tracker/reviews/publication.rs:136::review_update` | `self_handle`, `workspaces` |
| `tasks.review.merge` | `bridge/src/api/v1/reviews.rs::merge` | `bridge/src/app/tracker/reviews/merge.rs:57::review_merge` | `changes`, `deferred_work` |
| `tasks.review.close` | `bridge/src/api/v1/reviews.rs::close` | `bridge/src/app/tracker/reviews/lifecycle.rs:172::review_close` | none directly; see helper effects |
| `tasks.review.reopen` | `bridge/src/api/v1/reviews.rs::reopen` | `bridge/src/app/tracker/reviews/lifecycle.rs:207::review_reopen` | `self_handle`, `workspaces` |
| `tasks.review.refresh` | `bridge/src/api/v1/reviews.rs::refresh` | `bridge/src/app/tracker/reviews/lifecycle.rs:259::review_refresh` | `self_handle` |
| `tasks.review.snapshot` | `bridge/src/api/v1/reviews.rs::snapshot` | `bridge/src/app/tracker/reviews.rs:66::review_snapshot` | `deferred_work`, `workspaces` |
| `tasks.review.get` | `bridge/src/api/v1/reviews.rs::get` | `bridge/src/app/tracker/reviews.rs:128::review_get` | `deferred_work` |
| `tasks.review.act` | `bridge/src/api/v1/reviews.rs::act` | `bridge/src/app/tracker/reviews.rs:145::review_act` | `changes`, `deferred_work` |
| `tasks.review.diff` | `bridge/src/api/v1/reviews.rs::diff` | `bridge/src/app/tracker/reviews.rs:229::review_diff` | `deferred_work` |
| `tasks.review.complete` | `bridge/src/api/v1/reviews.rs::complete` | `bridge/src/app/tracker/reviews.rs:254::review_complete` | none directly; see helper effects |
| `tasks.list` | `bridge/src/api/v1/tasks.rs::tasks_list` | `bridge/src/app/tracker/mod.rs:148::tasks_list` | none directly; see helper effects |
| `tasks.get` | `bridge/src/api/v1/tasks.rs::tasks_get` | `bridge/src/app/tracker/mod.rs:225::tasks_get` | none directly; see helper effects |
| `tasks.create` | `bridge/src/api/v1/tasks.rs::tasks_create` | `bridge/src/app/tracker/mod.rs:294::tasks_create` | none directly; see helper effects |
| `tasks.update` | `bridge/src/api/v1/tasks.rs::tasks_update` | `bridge/src/app/tracker/mod.rs:356::tasks_update` | none directly; see helper effects |
| `tasks.comment` | `bridge/src/api/v1/tasks.rs::tasks_comment` | `bridge/src/app/tracker/mod.rs:374::tasks_comment` | none directly; see helper effects |
| `tasks.assign` | `bridge/src/api/v1/tasks.rs::tasks_assign` | `bridge/src/app/tracker/dispatch.rs:227::tasks_assign` | none directly; see helper effects |
| `tasks.waiting_on.add` | `bridge/src/api/v1/tasks.rs::tasks_waiting_on_add` | `bridge/src/app/tracker/waiting_on_api.rs:37::tasks_waiting_on_edit` | none directly; see helper effects |
| `tasks.waiting_on.remove` | `bridge/src/api/v1/tasks.rs::tasks_waiting_on_remove` | `bridge/src/app/tracker/waiting_on_api.rs:37::tasks_waiting_on_edit` | none directly; see helper effects |
| `tasks.waiting_on.blocks` | `bridge/src/api/v1/tasks.rs::tasks_waiting_on_blocks` | `bridge/src/app/tracker/waiting_on_api.rs:93::tasks_waiting_on_blocks` | none directly; see helper effects |
| `tasks.close` | `bridge/src/api/v1/tasks.rs::tasks_close` | `bridge/src/app/tracker/mod.rs:429::tasks_close` | none directly; see helper effects |
| `tasks.reopen` | `bridge/src/api/v1/tasks.rs::tasks_reopen` | `bridge/src/app/tracker/mod.rs:443::tasks_reopen` | none directly; see helper effects |
| `tasks.watch` | `bridge/src/api/v1/tasks.rs::tasks_watch` | `bridge/src/app/tracker/tracking.rs:62::tasks_watch` | none directly; see helper effects |
| `tasks.unwatch` | `bridge/src/api/v1/tasks.rs::tasks_unwatch` | `bridge/src/app/tracker/tracking.rs:70::tasks_unwatch` | none directly; see helper effects |
| `tasks.read_through` | `bridge/src/api/v1/tasks.rs::tasks_read_through` | `bridge/src/app/tracker/tracking.rs:122::tasks_read_through` | none directly; see helper effects |
| `tasks.columns` | `bridge/src/api/v1/tasks.rs::tasks_columns` | `bridge/src/app/tracker/mod.rs:277::tasks_columns` | none directly; see helper effects |
| `tasks.attach` | `bridge/src/api/v1/tasks.rs::tasks_attach` | `bridge/src/app/tracker/attachments.rs:69::tasks_attach` | none directly; see helper effects |
| `tasks.attachment` | `bridge/src/api/v1/tasks.rs::tasks_attachment` | `bridge/src/app/tracker/attachments.rs:109::tasks_attachment` | none directly; see helper effects |
| `thread.page` | `bridge/src/api/v1/thread.rs::thread_page` | `bridge/src/app/conversations/read.rs:288::thread_page` | none directly; see helper effects |
| `thread.revision` | `bridge/src/api/v1/thread.rs::thread_revision` | `bridge/src/app/conversations/read.rs:259::thread_revision` | none directly; see helper effects |
| `thread.activity` | `bridge/src/api/v1/thread.rs::thread_activity` | `bridge/src/app/conversations/read.rs:364::thread_activity` | none directly; see helper effects |
| `thread.post` | `bridge/src/api/v1/thread.rs::thread_post` | `bridge/src/app/conversations/post.rs:45::thread_post` | none directly; see helper effects |
| `thread.operation` | `bridge/src/api/v1/thread.rs::thread_operation` | `bridge/src/app/conversations/post.rs:672::thread_operation` | none directly; see helper effects |
| `thread.attach` | `bridge/src/api/v1/thread.rs::thread_attach` | `bridge/src/app/conversations/attachments.rs:286::thread_attach` | `store` |
| `thread.attachment` | `bridge/src/api/v1/thread.rs::thread_attachment` | `bridge/src/app/conversations/attachments.rs:358::thread_attachment` | none directly; see helper effects |
| `agent.add` | `bridge/src/api/v1/thread.rs::agent_add` | `bridge/src/app/runtime/agents/endpoints.rs:998::agent_add` | `cli_readings`, `default_harness`, `plans`, `runs` |
| `agent.choose` | `bridge/src/api/v1/thread.rs::agent_choose` | `bridge/src/app/runtime/agents/endpoints.rs:1181::agent_choose` | `plans` |
| `agent.list` | `bridge/src/api/v1/thread.rs::agent_list` | `bridge/src/app/runtime/agents/endpoints.rs:1430::agent_list` | none directly; see helper effects |
| `agent.remove` | `bridge/src/api/v1/thread.rs::agent_remove` | `bridge/src/app/runtime/agents/endpoints.rs:1308::agent_remove` | `board`, `changes`, `plans`, `runs`, `session_registry` |
| `conversation.watch` | `bridge/src/api/v1/thread.rs::conversation_watch` | `bridge/src/app/runtime/agents/records.rs:635::set_conversation_watched` | none directly; see helper effects |
| `conversation.unwatch` | `bridge/src/api/v1/thread.rs::conversation_unwatch` | `bridge/src/app/runtime/agents/records.rs:635::set_conversation_watched` | none directly; see helper effects |
| `conversation.reset` | `bridge/src/api/v1/thread.rs::conversation_reset` | `bridge/src/app/conversations/reset.rs:17::conversation_reset` | `cli_readings`, `state_root` |
| `conversation.settings` | `bridge/src/api/v1/thread.rs::conversation_settings` | `bridge/src/app/runtime/agents/records.rs:661::set_conversation_context_limit` | `compact_above_tokens` |
| `bridge.update_status` | `bridge/src/api/v1/updates.rs::update_status` | service helper in same v1 module | `updates` |
| `bridge.check_update` | `bridge/src/api/v1/updates.rs::check_update` | service helper in same v1 module | `updates` |
| `bridge.install_update` | `bridge/src/api/v1/updates.rs::install_update` | `bridge/src/app/updates.rs:53::update_has_working_agents` | `delivery_queue`, `plans`, `router_sessions`, `runs`, `session_registry` |
| `workspace.set_compaction` | `bridge/src/api/v1/workspace.rs::set_compaction` | `bridge/src/app/config/compaction_wire.rs:82::set_workspace_compaction` | none directly; see helper effects |
| `workspace.list` | `bridge/src/api/v1/workspace.rs::workspace_list` | `bridge/src/app/workspaces/mod.rs:343::workspace_list` | `workspaces` |
| `workspace.get` | `bridge/src/api/v1/workspace.rs::workspace_get` | `bridge/src/app/workspaces/mod.rs:394::workspace_get` | `workspaces` |
| `workspace.create` | `bridge/src/api/v1/workspace.rs::workspace_create` | `bridge/src/app/workspaces/mod.rs:640::workspace_create` | `deferred_work`, `isolation`, `workspaces` |
| `workspace.retry` | `bridge/src/api/v1/workspace.rs::workspace_retry` | `bridge/src/app/workspaces/mod.rs:861::workspace_retry` | `deferred_work`, `workspaces` |
| `workspace.ensure_conversation` | `bridge/src/api/v1/workspace.rs::workspace_ensure_conversation` | `bridge/src/app/workspaces/mod.rs:296::workspace_ensure_conversation` | `default_harness`, `projects`, `runs`, `workspaces` |
| `workspace.git_init_options` | `bridge/src/api/v1/workspace.rs::workspace_git_init_options` | `bridge/src/app/workspaces/git_initialization.rs:114::workspace_git_init_options` | `projects`, `workspaces` |
| `workspace.init_git` | `bridge/src/api/v1/workspace.rs::workspace_init_git` | `bridge/src/app/workspaces/git_initialization.rs:153::workspace_init_git` | `deferred_work`, `workspaces` |
| `workspace.finish` | `bridge/src/api/v1/workspace.rs::workspace_finish` | `bridge/src/app/workspaces/mod.rs:740::workspace_finish` | none directly; see helper effects |
| `workspace.add_directory` | `bridge/src/api/v1/workspace.rs::workspace_add_directory` | `bridge/src/app/workspaces/directories.rs:265::workspace_add_directory` | `deferred_work`, `projects`, `workspaces` |
| `workspace.remove_directory` | `bridge/src/api/v1/workspace.rs::workspace_remove_directory` | `bridge/src/app/workspaces/directories.rs:325::workspace_remove_directory` | `deferred_work`, `workspaces` |
| `workspace.rename` | `bridge/src/api/v1/workspace.rs::workspace_rename` | `bridge/src/app/workspaces/mod.rs:842::workspace_rename` | `active_deferred_filesystem_jobs`, `deferred_work`, `workspaces` |
| `workspace.set_locked` | `bridge/src/api/v1/workspace.rs::workspace_set_locked` | `bridge/src/app/workspaces/lock.rs:6::workspace_set_locked` | `active_deferred_filesystem_jobs`, `deferred_work`, `workspaces` |
| `workspace.delete` | `bridge/src/api/v1/workspace.rs::workspace_delete` | `bridge/src/app/workspaces/deletion.rs:273::workspace_delete` | none directly; see helper effects |
| `workspace.reclaim` | `bridge/src/api/v1/workspace.rs::workspace_reclaim` | `bridge/src/app/workspaces/reclaim/explicit.rs:78::workspace_reclaim` | `deferred_work`, `reclaim_policy`, `reclaim_stop` |
| `workspace.measure_sizes` | `bridge/src/api/v1/workspace.rs::workspace_measure_sizes` | `bridge/src/app/workspaces/sizes.rs:33::workspace_measure_sizes` | `size_requests`, `workspace_lifecycle` |


### 7.4 Session-scoped, legacy and QA arms

These are all 16 `api/mod.rs::LEGACY_METHODS`, all three `QA_METHODS`, and the
non-verb close-frame branch. Signaling is handled before session dispatch;
QA enablement remains construction-time state, not a new runtime switch.
M = in-memory read, S = SQLite-only read, W = write. I/O marks a read that
requires external observation. “Target M” means maintenance and scope lookup
have first been moved to published state. No legacy read is SQL-only today.

| Verb/arm | Classification | Write scope | Current AppState fields / service effects; implementation evidence |
| --- | --- | --- | --- |
| `ping` | M/static | none | `app/rpc.rs::pong`; intake fast path uses no AppState. Direct legacy dispatch still passes routing bookkeeping. |
| `bridge.stats` | M/metrics | none | `session_scoped` reads `FrameTimer::clock().stats()`, not AppState; current queue dependence is in §2. |
| `rtc.offer`, `rtc.ice`, `rtc.close` | W | device peer service, per session | `app/rtc.rs::{rtc_offer,rtc_ice,rtc_close}` uses the captured `peers` slot and mutable SessionPeers; no app mutex for signaling. |
| `session.hello` | W | device session subscriptions | `runtime/terminals.rs::session_hello`: `changes`, `qa_agent`, `updates`, `cli_readings`, `harness_inventory`, `update_subscriptions`, `models_subscriptions`, `harnesses_subscriptions`; FrameClock client registry records the declaration separately. Idempotent subscription is still a write. |
| `term.list` | M + scope I/O/possible maintenance now; target M | none after extraction | `runtime/terminals.rs::{term_list,shell_tabs_for}` reads `session_registry`; `terminals/scope.rs::resolve_terminal_scope` traverses `projects`, `runs`, `workspaces` and external checkout/cache resolution. The same legacy-adoption/cache caveat applies as for file scope reads. |
| `term.create` | W | project source or workspace; device terminal-cap reservation | `runtime/terminals.rs::term_create`: `project_deletion_in_progress`, scope fields above, `source_moves`/source guards, `reclaim_reserved`, `session_registry`, `term_shell`, `changes`; RPC success also touches `board`, `user_session`, `store`. It currently spawns shell/PTY under the app guard. Move spawning to reserve/work/apply without losing the cap or deletion recheck. |
| `term.attach` | W | session + resolved project/workspace terminal | `runtime/terminals.rs::term_attach`: `session_registry`, source/owner scope validation; cloned terminal handle attaches/resizes its screen off app lock. Screen subscription is the mutation even when the registry itself is only read. |
| `term.input`, `term.resize`, `term.ack` | W | addressed terminal/session within project or workspace | Same-named functions in `runtime/terminals.rs` resolve through `session_registry` and owner indices, then write PTY/resize/screen cursor off lock. These are not database writes and must not acquire the SQLite writer for bookkeeping. Preserve ordered lanes and detached-session behavior. |
| `term.close` | W | addressed project/workspace terminal | `runtime/terminals.rs::term_close` resolves `session_registry`/scope then `retire_tab`; registry, screen close/change effects and retirement. Agent tabs are refused. Exact retirement helper transitive field closure is an implementation audit gate. |
| `agent.attach` | W | canonical project/workspace agent + client session | `runtime/agents/endpoints.rs::agent_attach`: resolves `plans`, `runs`, project/workspace binding and thread guard; `session_registry` may allocate a waiting screen, then attaches screen clients. Never classify this as a read because its result is a snapshot. |
| `agent.start` | W | canonical project/workspace conversation | `runtime/agents/endpoints.rs::agent_start`: `plans`, addressed `runs`/owners/choices; `delivery_queue`, `board`, then `DeliveryRunner::drain`; RPC adds `user_session`/`store`/`changes`. Delivery's remaining fields are in R and require full closure audit before narrow admission. |
| `agent.interrupt` | W | canonical project/workspace conversation | `runtime/agents/endpoints.rs::agent_interrupt`: reads `plans`, binding/generation, `session_registry`; invokes the captured exact session's interrupt off lock. RPC success updates `user_session`/`store`/`changes`. Keep instance check across replacement. |
| `stream.start` | W, QA-only | device QA stream, then per stream | `app/streams.rs::stream_start` increments `next_stream`, inserts `streams`, background `append_event` mutates that stream. Reads `qa_agent` at dispatch. |
| `stream.events`, `stream.state` | M, QA-only | none | Same-named `app/streams.rs` methods read `streams`, gated by `qa_agent`; `&mut self` signature alone does not make them writers. |
| close frame (not a method) | W | ending session/device service ownership | `app/rpc.rs::dispatch_frame` → `runtime/terminals.rs::drop_session`: `uploads`, `peers`, `session_registry` screen detach; subscription maps for updates/models/harnesses, `changes`, `watchers`, FrameClock client registry. `FrameHandler::on_session_end` separately drops ended uploads. Preserve opening generation and ordering. |

### 7.5 Common effects and limits of the field audit

The grouped table plus direct-field appendix is a **known footprint**, not a
claim of a mechanically proven transitive write set. It deliberately reports
uncertainty where polymorphic deferred jobs, agent callbacks, notification,
retirement and rollback paths reach more helpers. Those paths stay on the
compatibility gate until their implementation patch enumerates and tests the
full closure. This plan does not silently assume that “none directly” means
no effects. The following common effects are part of every applicable row:

- RPC admission reads `updates`/its admission gate and
  `project_deletion_in_progress`; signaling uses the captured `peers` handle.
- Routed dispatch checkpoints `delivery_queue`; refusal rolls back queued
  turns. Deferred jobs use `deferred_work`, `deferred_result_check` and
  `active_deferred_filesystem_jobs`; lifecycle jobs also use `pending_rows`.
  Their apply phase can invalidate `board`, note `changes`, and refresh
  `watchers`. These become request-owned commands and published deltas.
- Successful `USER_ACTIVITY_VERBS` update `user_session`, may persist through
  `store`, and read `projects` to note task changes through `changes` when a
  session boundary moves. `INTERACTION_VERBS` update attention in `board` and
  its persistence. The exact lists in `app/rpc.rs` are authoritative; do not
  assume every mutation is on them (review mutations, for example, are not
  all listed).
- Plan/run persistence helpers reach `store`, `session_summaries`,
  `session_seen`, `board`, `changes` and `watchers`; rendering may reach
  runtime facts. Notification paths additionally use `notifier` and
  `notify_throttle`. Delivery/retirement may consult `agent`, `harness`,
  `session_locator_factory`, `resume_id_probe`, `live_roster`,
  `usage_limits`, `self_handle`, `push_runtime` and the R ledgers. This is a
  conservative helper inventory, not an assertion that every write touches
  every one of them.
- Reclaim/size/source-sync producers also own `reclaim_nudge`,
  `reclaim_policy`, `reclaim_settings`, `reclaim_stop`, `size_requests`,
  `workspace_lifecycle`, `reclaim_reserved`, `source_sync_requested`,
  `source_sync_nudge`, and `source_sync_stop`. Filesystem paths
  (`worktrees_root`, `projects_dir`, `state_root`, `bridge_exe`, `config_path`)
  and `qa_agent` are captured dependencies, not read by rediscovering daemon
  configuration inside read workers.

Target S-only candidates become S **after** validation/metadata extraction:
`tasks.waiting_on.blocks`, configured-store `thread.page`, `thread.activity`
and `thread.operation`. Tasks list/get retain M overlays unless those live
fields are deliberately projected into the same authoritative query; they
are M+S, not falsely labeled S-only. Git, filesystem documents, attachments
and external CLI reads are explicit I/O exceptions to the requested three
categories. This is why the table describes actual effects as well as target
routing. All read categories still obey the same no-app-lock/no-writer-queue
contract.
