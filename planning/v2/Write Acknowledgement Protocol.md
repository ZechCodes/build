# Write Acknowledgement and Bridge Epoch Protocol

Task #710, amendment to [Bridge Concurrency Plan](<Bridge Concurrency Plan.md>),
2026-10-20. **Proposed; planning only.** Baseline
`82b5ce0c47b3016fb237cd0a2db5376384d019c2`, wire 3.23.0, Store schema 17.
Names below are proposed unless identified as existing. This document does
not describe shipped behavior. Implementation releases and gates are in the
plan's steps 5–9.

## 1. Outcome and existing boundaries

A validated, durably accepted write resolves the user's action successfully.
The SPA paints that accepted change from its cache immediately. Execution
continues without a browser or MCP caller; a later confirmation settles the
canonical result. A reconnect reconciles operation identities, not request
timeouts. A bridge restart never causes an accepted operation to be submitted
under a new identity automatically.

Existing boundaries that the implementation must retain:

| Evidence in the baseline | Consequence |
| --- | --- |
| `bridge/src/carrier.rs::FrameIntake` emits `{id, accepted:true}` before application validation/durable admission; `spa/src/core/sessionRpc.js` extends its deadline | This is a transport receipt, never the new write ack. Do not rename or reinterpret it. |
| `store/operations.rs::accept_thread_post` saves transcript rows and a provider-delivery receipt together | Reuse this transaction for message acceptance. Its `queued/claimed/delivered/uncertain` states describe provider delivery, not generic mutation completion. |
| `store/delivery_intents.rs` and `delivery_intents/writes.rs` save task-assignment authority and generations | Keep these domain intents; a generic command links them rather than becoming another competing assignment worker. |
| `store/operations.rs::recover_operations` makes a claimed provider delivery uncertain | Never repeat a possibly delivered message because an outer ack/confirmation was lost. |
| `app/transactions/notices.rs` permits committed nested effects; `runtime/deferred.rs` validates a response after writeback | An error does not prove that nothing changed. Failure reconciliation needs an explicit effect outcome. |
| `app/mcp.rs::ControlPlane`, `Turn`, coding/router handlers | MCP reads and writes currently share permits/app locking; early replies would also remove their incidental execution ordering. |
| `api/effects.rs` exhaustively partitions 157 RPC names, including 107 `Write` entries | Extend this manifest, including session writes and mutating RTC controls; do not infer durability from `Write` alone. |
| `store.rs` uses WAL and `synchronous=NORMAL` | Initial durability means survival of bridge process death/restart, not a host-power-loss fsync guarantee. |

## 2. Durable acceptance and ownership

Add `mutation_commands`, `mutation_owner_order` and a durable confirmation
outbox in the existing database, using a forward-only schema migration
(schema 18 if it remains next). `store/mutations.rs` owns typed transactions;
`app/mutations.rs` owns admission/execution/publication. Both are new modules.
Keep one SQL writer connection, moved behind `store/writer.rs`; never enqueue
or await it while holding an application/domain mutex. Journal admission
cannot sit behind Git, a process spawn, a whole-history rewrite, or an
application compatibility reservation. SQLite's short commit serialization
remains; moving the wait behind a different global mutex is not a fix.

An accepted record contains:

- A caller-generated UUID `operation_id`, authenticated principal/device or
  MCP agent identity, semantic method/action, canonical normalized payload
  and hash, resolved target IDs, source/thread/owner generations and actor.
- A complete conflict-owner set, durable per-owner order, explicit bounded
  predecessor IDs, and stable IDs allocated for created resources and child
  delivery/assignment operations. No task ID, actor, authority or owner is
  trusted simply because it came from an envelope.
- Acceptance time/epoch, a monotonically increasing operation revision,
  execution/recovery policy, staged blob references where required, proposed
  cache effects and the eventual typed result or structured failure.

Acceptance checks authentication, schema/size limits, authority, immutable
scope, existing resource identity, input preconditions and admission quotas.
It saves enough intent to execute or conclusively refuse after a restart.
Mutable facts are rechecked at execution; admission is not a promise that a
later Git ref, file permission or free-space check will succeed. Filesystem
probes never hold an app lock. If a necessary admission probe is expensive,
the ack is not called fast until that probe is removed or independently
measured. No ack is sent on an in-memory queue insertion or failed commit.

Reserve created task/comment/workspace/agent IDs in admission. Files needed
after ack must already be in a bounded durable staging area, with the
existing containment and ownership rules; an ephemeral MCP attachment path
alone is insufficient. Refuse quota exhaustion before ack. Start with at
most 128 nonterminal commands per principal and 4,096 per bridge, with
separate byte quotas and reserved capacity for small presence/read-mark
writes. These are proposed engineering bounds, to be tuned by measurement;
write overload may return retryable `busy`, reads may not.

### Identity, retries and lifetime

The journal key is `(authenticated principal, operation_id)`, with a unique
internal command ID used by owner queues. Hash the normalized semantic
request and its original bound targets, excluding only transport metadata.
A retry resolves the existing key *before* resolving mutable aliases or
allocating children. Same intent returns the saved ack/outcome; different
intent returns `operation_conflict` without effects. A new request `id`,
carrier, epoch or browser tab does not change the command.

For existing `thread.post`/assignment operation parameters, require the
inner ID to equal or be deterministically bound to the outer command; never
generate a fresh inner ID on retry. In particular,
`app/conversations/agent_to_agent.rs` currently generates an ID on execution:
move that allocation into durable admission. Distinct actors cannot adopt
one another's commands. Status reads enforce current authorization to the
original scope and expose no result by possession of an ID alone.

Retain nonterminal records until reconciled. Retain full terminal results
for 30 days; compact older records to durable identity/hash/outcome
tombstones, retaining the non-reuse fence for the device database lifetime.
An old retry returns `result_expired`, never executes again. Large bodies
can be evicted after their result window. Persist a journal-incarnation ID
in database metadata, paired with a store-generation manifest outside the
database. Supported restore/reset tooling must advance that manifest and
issue a new incarnation before clients are admitted; copying an old database
also copies its old in-database ID and cannot itself detect rollback. Unknown
commands from another incarnation require reconciliation. An operation the
client already saw acknowledged but which is now absent is always quarantined
as `outcome_unknown`, never resubmitted. Unmanaged raw rollback of both files
is outside the recovery contract and cannot promise detection or exactly-once
recovery of forgotten effects. The process-restart guarantee also does not
cover commit loss after host power failure under WAL/NORMAL.

### Ordering and execution

Use `accepted → executing → committed → confirmed` or a terminal failure.
`committed` is internal: canonical SQL and the command's applied marker are
one transaction. After that transaction, publish the prepared immutable
owner projections, persist the terminal outcome/outbox entry, then deliver
confirmation. A failed terminal-outbox save leaves the command committed
for recovery; it does not re-execute the canonical mutation. Startup rebuilds
committed projections before advertising readiness and reissues outcomes.

An owner queue orders conflicting commands by durable admission order.
Multi-owner admission adds all predecessors atomically, using the plan's
device → project → workspace hierarchy and canonical physical Git exclusions.
Ancestor intentions allow sibling workspaces to overlap; reserving a device
counter must not serialize every device command. Exclusive project deletion
fences descendants. No upgrades/partial-set waits; discovered additional
owners require a new validated reservation before effect. An earlier failure
unblocks a later independent command after reconciliation; explicit dependent
commands fail `dependency_failed` when their prerequisite failed.

MCP preserves caller order for conflicting commands after ack, plus explicit
dependencies when a command uses a resource just reserved by an earlier
ack. A caller cannot read a reserved workspace path as though provisioning
had finished. Terminal input/resize/close retain exact per-terminal stream
ordering across the new command service. Reset, deletion, reclaim and source
movement retain the existing generation fences.

For external effects, durably claim the step before crossing the filesystem,
Git, PTY, process or provider boundary. Use operation-owned staging paths,
expected before/after refs, stable spawn/delivery IDs and existing domain
recovery journals to distinguish unapplied from applied. Recovery policy is
per verb, not a generic replay callback:

| Effect family | Recovery requirement |
| --- | --- |
| SQL-only task/settings metadata | Canonical write + applied marker in one transaction; publish once recovered. Config files need their own staged replacement/reconciliation. |
| Transcript/assignment/agent delivery | Link existing operations/delivery intents; accepted transcript is the mutation result. Provider execution is subsequent domain state, not write confirmation. Preserve claimed → uncertain. |
| Git/reviews/workspace provisioning | Capture expected refs/versions and owned paths; use existing review locks/journals. Reconcile an ambiguous push/commit before any repeat. |
| Files and uploads | Save bounded blobs and offsets/hashes; compare expected destination identity. Never overwrite a changed file on recovery. |
| PTY input, interrupts and process starts | Bind exact session/instance. A claim whose effect cannot be proven becomes `outcome_unknown`; never replay into a replacement process. |
| Session subscriptions/attachments | Bind opening + epoch; an ended opening terminates unapplied work as `scope_ended`. Re-establish desired subscriptions as new session commands, not replay of old handles. |
| Bridge update | Confirm durable update-job admission with its stable job ID; later installation/restart outcome remains update-service state. Do not claim the new executable is running at ack. |

The failed confirmation includes `effect: "unapplied" | "partial" | "unknown"`.
Only `unapplied` permits removal of that operation's optimistic effects
without additional reconciliation. A partial/unknown outcome carries
authoritative affected records or refresh addresses and preserves evidence
until reconciled. There is no generic rollback of already written Git refs,
PTY bytes, messages or unrelated later edits.

## 3. Wire contract

Introduce `writes.ackConfirm`, `bridge.epoch` and `writes.status`, initially
for a negotiated list `ack_confirm_methods`. Every supported invocation
explicitly opts in with `write.mode`; advertising a capability alone never
changes a legacy caller's result shape. New result envelopes have their own
typed validators; the existing method's result validator applies to the
confirmation, not to the provisional ack. The adapter must not resolve an
old typed API Promise with a new ack object by accident.

Example request:

```json
{"id":"r-17","method":"tasks.comment","params":{"task_id":"task-A","body":"Ready for review"},"write":{"mode":"ack_confirm","operation_id":"5d9b2df9-cd22-4cc0-9c50-eef3c7c3301a","after":[]}}
```

The existing transport receipt may precede this durable ack:

```json
{"id":"r-17","ok":true,"bridge_epoch":"boot-B","write":{"phase":"ack","operation_id":"5d9b2df9-cd22-4cc0-9c50-eef3c7c3301a","revision":1,"state":"accepted","accepted_epoch":"boot-B","owners":[{"kind":"project","id":"project-A","sequence":42}]},"result":{"task_id":"task-A","comment_id":"comment-R","accepted_at":"2026-10-20T19:00:00Z","overlay":[{"kind":"task_comment","task_id":"task-A","comment_id":"comment-R","body":"Ready for review"}]}}
```

`result` here is an acceptance result, not the old full task detail response.
The typed overlay names allowed cache effects and reserved canonical IDs.
It is never arbitrary JSON patches or executable instructions. Server
normalization wins over the local draft. Ack settles the action Promise,
closes the editor, and paints the accepted row as successful; it does not
leave a spinner pending execution. No extra success toast is necessary.

After application and publication:

```json
{"type":"writes.confirmed","bridge_epoch":"boot-B","operation_id":"5d9b2df9-cd22-4cc0-9c50-eef3c7c3301a","revision":2,"state":"succeeded","result":{"comment_id":"comment-R","task_id":"task-A"},"published":[{"kind":"project","id":"project-A","sequence":42}],"refresh":[{"kind":"task","id":"task-A"}]}
```

This abbreviated example omits the verb's full existing result. Implementation
fixtures must use its complete typed result. Confirmation includes either
canonical records with their revisions/generations or precise refresh
addresses plus publication fences. An invalidation alone cannot clear an
overlay before those records arrive. For a failure:

```json
{"type":"writes.confirmed","bridge_epoch":"boot-B","operation_id":"5d9b2df9-cd22-4cc0-9c50-eef3c7c3301a","revision":2,"state":"failed","effect":"unapplied","error":{"code":"stale_generation","message":"The task changed before this comment was saved.","retryable":false},"refresh":[{"kind":"task","id":"task-A"}]}
```

`writes.status` is a bounded authenticated read of up to 128 operation IDs,
returning latest ack/terminal summaries, including `not_found`/`result_expired`.
Cap total encoded results at 512 KiB; return a stable cursor when summaries
do not fit. Large final results use revision-bound result references fetched
through a paged `writes.result` read, never 128 unbounded embedded histories.
Both reads use the independent SQL read lane and its sender credits.
A `not_found` result in the same journal incarnation, for an operation
**never acknowledged to this client** and under the documented intact-store
process-restart contract, permits retransmission of the exact original
intent with its same ID: duplicate admission is atomic even if an older send
arrives late. An acknowledged-but-missing record, restore/integrity
uncertainty or changed incarnation requires reconciliation.
It does not permit generating a new ID. A status read never executes work.

`writes.subscribe` registers an authorized bounded operation set, atomically
establishes a cursor and returns current states. Then it streams later
outcomes; this closes the status/subscribe race. Confirmation delivery is
at-least-once and independent of the 250 ms ordinary change coalescer.
Duplicates/out-of-order responses cannot regress an operation revision.
Fast writes still send the ack and confirmation separately in v1 of this
protocol, possibly consecutively; combining them is a future optimization
requiring a fixture and negotiated semantics.

### Read-your-own-write

After **ack**, canonical reads can still return the previous committed
value; waiting for execution would break the read budget. The origin SPA's
effective cache overlays all accepted commands in owner order on the latest
canonical base, so its reads/paint retain the accepted edit. Other clients
receive only committed facts unless they explicitly subscribe to an
authorized operation. Ack does not make a speculative change globally true.

After **successful confirmation**, a fresh canonical SQL transaction or
snapshot load sees the published write or a later revision. Before confirming
file/Git/review mutations, advance affected observation fences and either
publish their new values or mark earlier observations stale. Cached
`fs.read/tree`, Git status/diff and review results older than that fence may
remain as explicitly stale display data; they cannot be returned as current
or retire the accepted overlay. A current-value read returns the new revision
or explicit pending state, never an apparently current pre-write observation.
A request
dispatched after confirmation cannot join a running pre-confirm read.
Responses already in flight carry scope/generation/revision fences and
cannot overwrite a newer base. RPC/MCP consumers needing applied data use
status/confirmation or an explicit command dependency; no read waits on an
operation. This deliberately changes the old plan's use of “post-ack”:
its canonical visibility guarantee now attaches to confirmation.

### Every write, including already-fast/session work

The plan's §5 policy table is exhaustive. “Already fast” is an execution
target, not permission to emit an undurable ack: a short command can finish
application before sending ack, then send the saved confirmation immediately.
All opted-in domain/session write RPCs and MCP writes use the same envelope.
Session-bound commands still record an intent/claim/outcome; their recovery
can end in `scope_ended` or `outcome_unknown`, never replay of stale handles.

For terminal high-rate input/flow control this literal all-writes rule adds
durable writes. The initial proposal retains it, uses bounded batched journal
commits with per-terminal order, and measures input/read latency under load.
It supersedes the old plan's “no SQLite bookkeeping” terminal sentence only
for negotiated journaled commands; SQL is still never acquired under an app
or terminal guard. A recommended narrower exception is an explicit product
decision in §8, not a silent exemption. `session.hello`/`rtc.*` needed to
bootstrap negotiation keep their existing synchronous semantics; optional
post-negotiation invocations can opt in. The close transport frame is
cleanup, not a retryable write RPC.

## 4. Bridge epoch and restart recovery

Generate one random `bridge_epoch` per daemon start; all sessions/carriers
in that process share it. Separately persist `journal_incarnation`.
`session.hello` returns both and the negotiated capabilities. Supported
database restore/reset rotates the incarnation via the external store
manifest described in §2; a boot epoch alone never proves journal integrity.
Stamp every
authenticated application reply, receipt, write ack, confirmation and push
(including changes, terminal and service pushes) from the sending bridge
instance. The transport envelope owns this, not every producer independently.
Signaling replies also carry the epoch when authenticated; relays do not
invent or interpret it. Add a best-effort `bridge.restarting` event before
planned update handoff, but never depend on that event arriving.

`spa/src/connection.js::landSession` handles authoritative greeting/restore.
Compare epochs for the authenticated device and current connection lifetime:

1. Same epoch: a path/carrier changed; reconcile outstanding operations
   normally. Do not conclude that execution restarted.
2. Different epoch: old wire requests/ack timers/subscriptions are dead.
   Keep durable client operations and their accepted overlays. Query status
   and re-subscribe on the new opening before retransmitting unknown IDs.
3. An old/different-epoch frame cannot change cache or a new pending request.
   Treat an unexpected epoch as a reason for an authoritative re-greeting,
   not proof that the uncorrelated frame is newer. UUID epochs have no order.
4. New journal incarnation or incapable downgrade: retain pending records
   and do not replay them through legacy write APIs. Reconcile through a
   compatible bridge or expose an operation-specific unresolved result.

The current product's relay is rendezvous/signaling only; application RPCs
ride direct or TURN-relayed WebRTC DataChannels. Apply the same authenticated
epoch check to both paths and all replacement carriers. Do not enable writes
over the signaling relay. Carrier migration alone is not a bridge restart.
Multiple tabs/devices reconcile their own authenticated operations; duplicate
submissions of one ID have one execution. Subscribe fanout checks each
client's authority, and accepted metadata is not broadcast device-wide.

A crash after acceptance but before ack is recovered through the same ID.
A crash after ack but before apply resumes safe accepted work. A crash after
effect claim reconciles by that verb's policy. A crash after SQL commit but
before publication rebuilds/publishes the committed result before confirming.
A crash after confirmation persistence but before delivery replays the saved
confirmation. A startup notice is only observable once communication returns;
the disconnected SPA simply keeps rendering its effective cache.

## 5. SPA pending operations and failure UX

Add `core/writeOperations.js` (submission, status/retry, dependencies),
`core/writeOperationCache.js` (IndexedDB operations and effective projections)
and `core/bridgeEpoch.js` (epoch authority/fences). Extend `localCache.js`
with a versioned pending-operation store, keyed by account/device/principal
and operation ID. Persist the normalized request, owner/generation, draft,
original base revision, ordered overlay, reserved IDs, accepted epoch,
journal incarnation and latest operation revision **before sending**.
If persistence fails, keep the draft and refuse to claim a reload-safe send.

Keep canonical cache records separate from optimistic layers; all view
selectors read their composed cache projection. Rebase pending layers on
new canonical pulls/pushes rather than overwriting them. Use IndexedDB
transactions and BroadcastChannel notifications for atomic tab-visible
updates; a per-device tab lease suppresses duplicate transport, while server
idempotency handles lease races. Reload restores operations and overlays
before rendering; greeting controls dispatch, not whether accepted data paints.

| Input | Cache/action transition |
| --- | --- |
| Local send | Persist intent; optional existing local optimistic draft may paint, but do not claim acknowledged success. |
| Transport receipt | Keep awaiting durable ack. No business-state transition. |
| Durable ack | Install normalized accepted overlay/reserved IDs atomically; resolve action as success. |
| Timeout, carrier loss, epoch change | Retain intent/overlay; status/retry with same ID. No rollback or connection banner. |
| Successful confirmation | Install newer canonical records; remove the layer only when its published revision is present. If only invalidation arrived, retain it through the refresh. |
| Failed, unapplied | Remove only this layer, rebase subsequent layers, restore the relevant draft and cache a dismissible operation error. |
| Failed, partial/unknown | Refresh the named canonical state, retain an unresolved-operation record; never restore a whole stale before-image. |

Missing acks or confirmations also reconcile without an epoch change.
One per-device worker batches outstanding IDs into bounded status reads,
with jittered 1/2/5/10/30-second backoff, reset by an authoritative greeting
or outcome. Timers schedule status work; they never turn an accepted command
into failure or create a replacement ID. Cancel local waiting independently
of durable execution; an execution-cancel feature would need its own
idempotent command and outcome, not deletion of the pending cache row.

Rare failure copy belongs beside the affected action, for example “Your
comment wasn’t saved. Retry” with the draft intact. A terminal refusal retry
is a new user action with a new ID after current preconditions are fetched.
Uncertain delivery's “Check again” performs status/reconciliation on the same
ID, never a new write. Background presence/read-mark failures silently retry
only when proven unapplied and still relevant; they do not show action noise.
Disconnect itself is not a cached failure.

Integration points: `sessionRpc.js` distinguishes receipts/acks/final replies
and routes late confirms to the persistent service; `session.js` and
`connection.js` own carrier/lifetime boundaries; `changeEvents.js` registers
the new pushes; `bridgeApi/v1/index.js` and per-family adapters enforce current
greeting/method support; `optimistic.js` must stop dropping overlays on every
uncertain error. Reuse `chatRepository.js` operation-identity reconciliation,
but its current in-memory Map is not reload persistence. Migrate thread
submission into the shared store without duplicate optimistic message rows.

## 6. MCP and agent delivery

`mcp.control` is a local control-plane timing label, not a public RPC method.
`agent.deliver` is likewise the internal hot delivery path, not an entry in
the current v1 registry. Cover it through `MessageAgent`,
`MessageWorkspaceAgent`, `thread.post` and assignment delivery; do not add a
new public verb just to match a timing label.

Add an exhaustive policy match over `mcp.rs::BridgeAction` plus coding/router
`DoneReport`. Read actions bypass `ControlPlane::Turn` and app-lock write
admission through captured authenticated contexts, with current thread
generation validation. Write actions use the same journal and owner
scheduler as RPC. `mcp.rs` tool schemas and control client carry an optional
caller-supplied `operation_id`. An authenticated control handshake negotiates
the daemon's epoch/incarnation, `writes.ackConfirm` and exact action/report
support. Each control frame opts in explicitly; without handshake/opt-in the
daemon returns the legacy final result. Test old adapters/new daemons and the
reverse, including an adapter surviving a daemon downgrade.

The adapter persists a random harness-session namespace and a mapping from
stable provider invocation identity to one operation UUID before sending.
Preserve `run_stdio`'s current `action_id` through the adapter, but do not
treat a reusable JSON-RPC request number as a stable provider invocation.
If the harness supplies no stable identity across adapter restart, reuse
only the adapter's persisted outstanding-send record for transport retries.
Require explicit `operation_id`/status reconciliation for a later ambiguous
re-invocation; do not infer that identical arguments are a retry. Two
intentional identical comments must remain distinct. Never allocate a fresh
ID merely because the control socket reconnected.

At ack, MCP returns one ordinary successful tool result with:

```json
{"ok":true,"phase":"ack","state":"accepted","operation_id":"5d9b2df9-cd22-4cc0-9c50-eef3c7c3301a","bridge_epoch":"boot-B","result":{"task_id":"task-A","comment_id":"comment-R"},"confirmation":{"tool":"get_write_operation","operation_id":"5d9b2df9-cd22-4cc0-9c50-eef3c7c3301a"}}
```

The tool text says the write was accepted and gives stable reserved IDs;
it does not say provisioning, shell execution, merge or provider delivery
finished. `get_write_operation` is a read-only status tool. An established
control notification stream carries later outcomes to the MCP adapter,
which exposes notifications where supported and otherwise retains them for
status reads/next tool context. Do not send a second JSON-RPC tool response
for the same call or create an agent turn per successful background write.
Failure notices use the existing durable agent-notice machinery when the
agent must act. Agent-triggering followups run from committed domain
state, not from a speculative success token.

Completion reports currently return no response; add a negotiated ack
channel and durable report ID, keep the agent/session alive until admission
is known, and preserve task completion-authority fences. Tool actions that
change Complete/Blocked status may resolve the model call at ack, while
bridge finalization happens only after committed execution. Old adapters
continue awaiting the old final result. Mutable nested writes and router
actions are included, not just the common tracker tools.

## 7. Compatibility, tests and rollback

Minor versions suffice because envelopes are explicitly opt-in and existing
requests/replies are preserved. Allocate 3.24–3.26 as described in the plan
only if still available at implementation time; check then-current main,
advance once per release and update `API_VERSION`, version fixtures, method/
event fixtures, announced capabilities and SPA capability adapters together.

Test first, then implement each slice:

- Duplicate same ID, conflicting payload, another actor, renamed alias,
  deleted/reset owner, stable reserved/child IDs and explicit dependencies.
- Terminate a disposable test worker at each acceptance/claim/effect/commit/
  publication/outbox boundary; reopen its temporary Store and prove the
  documented recovery. No installed bridge or real provider/Git remote.
- Lost/reordered ack and confirm, confirm preceding ack at the client,
  retransmit over another carrier, same-epoch reconnect, abrupt/new-epoch
  restart, planned notice loss, stale restore callback, journal replacement.
- Unsubscribed client, reconnect subscribe/status race, two authorized
  clients/tabs, unauthorized guessed ID, duplicate outbox delivery and expiry.
- Failed save and nested partial success, publication after disconnected
  caller, failed terminal-outbox save, retained canonical revision and
  per-operation rollback without undoing subsequent edits.
- Reload before send/after ack, offline cached paint, IDB failure, cold mount
  before greeting, old-client/new-bridge and new-client/old-bridge combinations.
- Full MCP action/report manifest, tool result at ack, sequential dependent
  calls, notification fallback, auth/thread replacement and internal delivery.
- Terminal flow-control/input throughput with journal batching, session
  teardown, uploads, Git/review ambiguity and no duplicate external effects.
- Plan step-9 read budget under journal admission/execution load; separately
  report ack latency (target p99 ≤100 ms for hot bounded commands) and time
  to confirmation. Never use receipts as ack timing.

Rollback disables **new** early-admission methods through the advertised
allowlist. Keep status, recovery workers, confirmation outbox and pending
cache support until every previously accepted command is terminal. A
schema-unaware binary cannot be rolled over outstanding commands: use a
forward-compatible build or drain first. A method can return to synchronous
execution behind the same journal/protocol without changing identities.
Never turn accepted operations into legacy retries or drop the journal to
make a rollback appear clean.

## 8. Decisions for Zech before the affected implementation slice

1. **Real-time/session controls:** the literal all-writes plan above journals
   negotiated terminal/subscription commands; bootstrap signaling stays
   synchronous. Recommend an explicit exception for `rtc.*`, `session.hello`,
   `changes.*`, terminal attach/input/resize/ack and transport close: keep
   their ordered, session-bound final replies and epoch fences, with no
   durable early ack or automatic replay. These effects cannot survive their
   process/session and high-rate journaling adds avoidable latency. This is
   a requested scope amendment, not an assumed approval; domain writes,
   terminal creation/closure and all MCP semantic writes remain covered.
2. **Meaning of ≤100 ms:** recommend p99 ≤100 ms for complete bounded local
   read results in a declared load envelope, with max ≤100 ms in controlled
   held-lock tests, reporting all outliers. New paged/cached observation
   reads answer cold misses explicitly and refresh asynchronously. Arbitrary
   fresh network/Git data and network transfer have no honest 100 ms maximum.
   If the requirement is a universal end-to-end maximum, fresh unbounded
   legacy reads must be retired in a major version; no current measurement
   or timeout change can establish that promise.
3. **Durability scope:** recommend bridge-process restart durability first,
   matching the stated problem and existing WAL/NORMAL policy. If ack must
   survive host power loss, require FULL/group-fsync admission plus durable
   staged files before ack, and measure that cost before enabling the flag.
   This is a release policy decision, not something to hide behind “saved.”
