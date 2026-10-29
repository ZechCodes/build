# #925: bounded exact List and Board selection builds

Design/measurement proposal, 2026-10-29. **No production implementation,
capability, activation, or cost acceptance is claimed.** Base:
`fcec698761695148bcd8736e9bb516aefa00a8e0`, freshly fetched `origin/dev`.
The managed checkout initially used main `985569322`; it was moved to the
requested base before changes. No newer dev was selected.

This is the separate #887 prerequisite for #891's exact selected total, three
disjoint List groups and ordered heads, filtered Board status discovery, column
counts and readiness. #900 remains closed. #901 detail/history, search,
journal/bootstrap (#892), and transport are outside this proposal. Nothing
changes canonical write acceptance or legacy reads. The attached #891 matrix,
including its relation-budget clarification, is the semantic authority.

## Decision to review

Use an explicitly admitted backend service that builds private selected
membership and metadata across short transactions, then atomically publishes
one complete generation. Admit only sealed caller inputs and exact source
facts, or refuse before publication. Prefer a **project-scoped task-projection
generation** over today's store-wide basis, subject to a complete producer
coverage audit and measured producer costs. Neither choice supplies a retained
snapshot or guarantees progress under mutation of the selected project.

The service owns scheduling; polling observes progress and never advances work.
There is no client cursor-drain loop, eager active-query fanout on writes, or
implicit first-sync activation. New scopes remain pending/unavailable until a
complete exact answer exists. Last complete cached results retain their actual
query, roster and basis tags. A pending scope is never an exact zero.

Adoption requires lead disposition of this evidence, separate activation and
producer/storage cost decisions, versioned wire fixtures, production tests,
independent scope/code review and final Astra review. Prototype constants are
research guardrails, not validated bridge latency or production quotas.

## Exact identity and semantics

Authority identity consists of authenticated device/store incarnation, project,
semantic version, view, normalized selectors, attention rule, sealed known-agent
set K, separately sealed workspace residence set W (including explicit empty
versus absent), resolved coordinator presence/id plus interpretation version,
readiness interpretation, and configured-column identity when server metadata
classifies configured columns. The server returns an opaque immutable identity;
clients do not assert equality by hash alone. Changing any component fences all
old responses. A scope covers exactly one project; no unbounded epoch vector.

Within each state/status/label/assignee selector use OR; AND dimensions. Board
removes user status selection before sealing its filter. Exact assignee means
actual assignee only. `any` means actual-assignee presence OR pending-target presence; `none` its
complement. Presence is not the existing `held` fact: a retained pending target
with an empty/malformed kind can be present while pending_held is false. Carry
pending_present explicitly and fixture that distinction. W restricts to actual explicit agent assignment in W and intersects
user selectors; linked/tracked workspace tasks and project_agent do not enter W.
Known idle or blocked agents remain known. Unknown assignees are still selected
when F allows them. Cached project roster rows, not a fresh server roster, own K.

Coordinator resolution is frozen by the caller from its cached owner row. A
complete modern roster uses its persisted pin only; an absent target resolves
no coordinator. Legacy resolution uses working agent then first agent. The
server accepts this interpretation as input, authenticates its scope and exact
set, and does not infer a newer roster. Owner absence is explicit. Modern
working/order changes alone do not change K or coordinator resolution.

For selected F, U is state not Closed and status not Done. Let A be the canonical
asked or legacy attention fact (already implies U). G1=F intersect U intersect
known holder; G2=(F intersect A) minus G1; G3=F minus G1 minus G2. Project_agent is
a known holder only when the frozen coordinator exists in K. Each candidate
contributes once. Store counts G1/G2/G3 and total, assert their sum without a
terminal scan. List stream order is group rank then number descending. A page
is a prefix of that stream. One visible budget (initially 25) spans groups;
collapse hides rows without redistributing slots. No later group fills an
unknown earlier head.

Asked attention's mention floor is read_through. Watched unread uses the maximum
of read/watch floors. They are separate facts; never combine them while merging
candidates. Legacy attention retains its existence/unread/dismissal gates.
Prototype inputs take attention booleans, not fabricated unread approximations;
production integration must reuse canonical #900 attention semantics.

Board configured columns retain configuration order and exact zeros. Selected
stray statuses follow descending maximum selected task number, preserving first
occurrence in number-descending F, not lexical filter-choice order. Counts and
maxima accumulate once per selected task. Discover all statuses through bounded
pages with explicit completion. No 100/500-status truncation. Non-Backlog cards
use number descending. A configured Backlog column uses Blocked then Ready, priority rank descending
then number descending, iff any selected Backlog has readiness metadata.
Readiness presence is distinct from blocked=true. Modern summaries always carry
a dependencies object, so versioned modern readiness activates any nonempty
Backlog. Historical absent-readiness semantics retain plain number order.
A stray status named `backlog` remains ungrouped when no configured Backlog
exists, even if it has readiness metadata. Unknown priorities rank below low, as in the current model.

## Source domain and refusal boundary

The current sources are not a lossless arbitrary-domain catalogue. Source
anchors at the exact base: `bridge/src/store/task_summaries/query_projection.rs`
(TABLES, basis, refresh/relations), `query_sources.rs` (task_written), `sql.rs`
(base summary invalid flags), `maintenance.rs` (producer hooks), `query_reads.rs`
(CanonicalSelection), and `spa/src/core/trackerBoardModel.js`.

| Canonical input | Existing projection behavior | Minimum safe proposal |
| --- | --- | --- |
| Status 0..128 UTF-8 bytes (empty allowed for legacy stray) | Exact source bytes; ordinary status parser accepts built-ins only | Discovery/card membership uses exact opaque status, no built-in parser; case, NUL and Unicode preserved |
| Status >128 bytes | Source sentinel plus status_truncated; distinct values could alias | Typed indexed invalid flag; refuse applicable scope before complete, never a sentinel column |
| Labels <=20, each nonempty/trimmed <=40 bytes | Bounded source JSON, exact for accepted domain; display projection clips | Read at most 20 exact source values; use canonical matching normalization, not display strings |
| More labels or a longer label | Source becomes []; summary invalid flags retain unsupported-domain signal | Refuse with exact reason or build a new lossless label relation projection; never conclude no match from [] |
| Actual/creator actor or task/reference IDs >128 bytes | Some source IDs become NULL; summary carries invalid references | Indexed typed refusal flags before exact publication; never treat NULL as unassigned/unknown |
| Actor/status sets >100 entries | Current wire plural arrays cannot carry unlimited sets | New staged immutable sets; no membership truncation; capacity refusal is distinct from size/domain validation |
| Distinct statuses >500 | Could approach selected-task cardinality | Incremental status index and independent metadata pages; no fixed cardinality cut |
| Canonical task number outside exact wire/SQLite domain | Existing producers can saturate u64 to i64::MAX | Refuse ordering domain before seal; never merge/saturate distinct canonical numbers |
| Unbounded markers/timestamp forms | Explicit query invalid flags already exist for several fields | Reuse precise invalid flags only for semantics that need them; no lossy parsing fallback |
| Workspace/tracker relation values | Existing membership rows lack byte CHECK; no label relation exists | Do not claim bounded bytes from LIMIT alone; validate/source-bound new facts before paging |

Empty/untrimmed labels within the cardinality/byte ceiling can remain in the
source JSON while summary invalid-label flags mark them unsupported. Do not
interpret that JSON alone as supported. State and readiness-presence are not
separate compact columns today; new bounded headers or a reviewed bounded
extraction must supply them. A row LIMIT over unchecked task/project IDs is
not a byte bound: use byte-safe length/invalid expressions before allocation
or producer-constrained exact fields.

Lengths are UTF-8 bytes including bytes after NUL, not SQLite text length or JS
UTF-16 length. Opaque values receive no trim/case folding. Labels use the legacy
ASCII normalization rule, separately versioned from opaque identifiers. A
future exact relation domain can enlarge supported label cardinality while
bounding each fact (e.g. 128 bytes); larger facts remain explicit unsupported
input, not clipped data. Preserving canonical legacy data means an overlong
write still commits normally; it sets the selection support flag instead of
making the canonical write fail.

Two safe source alternatives need a delivery decision:

* **Refusal first:** hoist existing summary invalid signals into bounded typed
  facts with an index `(project, invalid_kind, number)`. Preserve exact bounded
  inline labels. Conservatively refuse the whole project scope for relevant
  invalid kinds via indexed seeks. This may refuse an off-filter task; say so.
  A more selective refusal may be decided during the complete bounded scan,
  only after enough lossless cheap predicates prove a candidate outside F.
  Uncertainty means refuse. Invalid off-page candidates must still be visited
  before seal. A filter cannot exclude an unknown value using its sentinel.
* **Exact new facts:** producer-maintained compact headers and normalized label
  relations, byte constrained and indexed `(task, value)` with cursor tie keys.
  Maintenance derives them from the already decoded canonical write in the
  same transaction. All actual labels survive, or an invalid flag survives.
  This adds O(L) relation work/storage per task write and migration. Replacing
  one task's huge relation via whole-task DELETE is not a bounded build phase;
  a production producer/activation plan must own this separately rather than
  hiding it behind selection advancement.

Recommendation for a first production proposal is refusal first; the research
relation prototype explores the larger exact-facts alternative. Neither is
implemented here. Backfill/repair must be resumable and keep capability support
pending until coverage is complete. No full-project canonical JSON scan on
admission; no giant JSON extraction on a worker quantum; no failure to find an
invalid flag in an incomplete backfill as evidence of support.

## Proposed operation concepts (not registered RPC names)

A new independently negotiated selection-build version is required; existing
`tasks.listSummaries`, counts/filter choices and `tasks.listPaged` do not imply
support. Old clients/bridges retain existing behavior. A capability describes
supported semantics/domain, not per-project readiness. Responses store support
state in cache; rendering never branches on connection state. Final names and
wire minor version belong to the implementation review.

| Operation | Input and output | Boundedness/authority |
| --- | --- | --- |
| Open set | scope, type, declared unique count; set handle | One fixed metadata row and quota reservation; owner-bound handle |
| Append set | set, monotonic chunk sequence, entries; accepted count/checkpoint | <=100 entries and <=64 KiB encoded; bounded dedup point probes; duplicate chunks idempotent, sequence/hash mismatch conflict |
| Seal set | handle, expected count; immutable seal | Incrementally maintained unique count/digest and O(1) state transition; no roster sort/COUNT/hash scan |
| Admit build | fixed-size specification with sealed set references; pending build | <=16 fixed set references; O(1) indexed validity/capacity checks, no roster enumeration |
| Observe | build; state, progress, retry policy | At most one metadata record, <=8 KiB; progress counters explicitly non-exact |
| Read totals | complete build identity; total/G1/G2/G3/readiness | O(1) stored counters; only `complete` has exact totals |
| Read columns | build + opaque continuation; <=100 records/64 KiB | Configured order/zero slots and stray maximum-number index; discovery complete separate from card completion |
| Read members | build, stream/column, cursor; membership positions and bounded summaries | <=500 rows/1 MiB, defaults 100; seeks indexed selected membership; never arbitrary filtered aggregate |
| Cancel | build or unsealed set; terminal state | One state update releases scheduling; storage remains charged until bounded GC |
| Retry | refused/restart-required/expired identity; new generation | Explicit user retry after finite automatic budget; does not relabel cached generation |

Example pending response: `{state:"building", identity, basis, progress:{headers,
relations}, exact:null}`. Complete returns `{state:"complete", identity, basis,
total, groups, columns_cursor, list_cursor}`. Refusal returns
`{state:"refused", reason:"unsupported_source_domain"|"capacity"|"budget",
retryable, retry_after_ms?, identity}` with **no success totals/cursor**. Other
states: staging, queued, restart_required, cancelled, expired. Reached EOF with
zero selected is the only exact zero. Store replacement/expired seals, changed
filter/roster and malformed/future/foreign cursors get explicit distinct errors.

Handles/cursors are authenticated owner-bound opaque tokens <=2 KiB encoded;
never accept client SQL/order identifiers. Bind cursor MAC or unguessable server
record to incarnation, identity, generation, kind, stream and exact keyset
position. Production must reuse established authentication/ownership and token
key management, with wire fixtures for replay, cross-project/owner reuse and
expiry. Bounded parse before lookup; no token embeds the roster or all statuses.
A roster can be staged in any arrival order; its immutable seal denotes the
exact stored set, not a claim of canonical serialization. Append idempotency
records consume quota and GC too. Final seal compares maintained count with
declared count; duplicates never inflate it.

## Worker, transaction, output and cleanup

One owned background worker, off the app lock, advances admitted jobs fairly.
A quantum checks generation, reads keyset headers, evaluates scalar predicates
with sealed-set point probes, and consumes multivalue facts with a separate
relation cursor. When a candidate is unfinished, checkpoint it; a partial
negative match is not absence. Positive OR may short-circuit. Indexes and
counters update incrementally as each selected candidate becomes decidable.
No unbounded EXISTS, GROUP BY, ORDER BY temp sort, index creation or COUNT at
finalization. Persist candidate/label cursor, membership, status counters and
job progress in the same short transaction. On interruption roll everything
back, return budget/refusal state via a separate O(1) transaction; never skip
an offending candidate. Retry a VM-failing quantum at most once with a smaller
fixed budget; persistent nonprogress refuses, rather than looping.

Candidate headers include state, exact status/kind/actual agent, unfinished,
attention facts, blocked, readiness-present, priority and exact number. Header
and relation reads do not fetch bodies, history, identity dictionaries or full
summary JSON. Membership stores task ID/number/group/status/priority/blocked;
private indexes support List rank+number and Board number+priority variants.
Use stored normalized sort-key columns (e.g. negative number/priority/blocked)
with full-key indexed tuple seeks. An expression index with a tuple containing
`-number` can seek only a prefix on the installed SQLite and scan earlier rows
in a group despite LIMIT. Verify EXPLAIN bytecode/query plan and bounded VM
counts for late pages at growing cardinality; index presence alone is not proof.
Status maxima use a stored normalized max-key and status tie key for stray
paging. First-page cursors have an explicit absent position: empty status is a
valid stray identity, not a cursor sentinel.
Store status max/count and readiness while writing membership. Configured columns are an ordered sequence, not a mathematical set: stage
(ordinal, exact status ID) rows with contiguous ordinal checkpoints and seal the
sequence count in O(1). Keep a separate deduplicated status-presence index for
point probes. Preserve repeated configured IDs as separate display slots (the
current columnsOf retains them), each with the same exact status count; do not
sum column-slot counts as the selected total. A zero-length normalized sequence
uses the same versioned fallback columns as columnsOf; freeze that resolved
sequence before admission. Names remain separately cached presentation. A
sealed config identity binds sequence and fallback interpretation. Thus an
unordered roster/filter set handle cannot substitute for column configuration.
A separate configured-column sequence can be staged; avoid one finalize join against unbounded
configuration. Configured presence can be point-probed while each status is
encountered; empty configured slots are read from the sealed set with zero
lookup defaults. Finalize checks B and flips one job state atomically.

Output membership, totals and column metadata compose only at identical query,
roster and task basis. Loading card summaries requires the same task-projection
basis in a short read transaction. A changed basis returns restart_required
before appending anything. No cross-quantum transaction, retained WAL reader,
snapshot lease or mutation replay is invented. Client cache publication installs
metadata, membership, fetched card versions and continuation atomically for one
identity/basis after checking the local mutation/request epoch. Retain immutable
query-owned copies (or version references) of **already loaded** card payloads at
B1, charged to the client cache quota, so later normalized B2 writes cannot
silently replace B1 cards beside B1 totals. Rows without a matching retained
version remain unavailable; never read a B2 normalized row as a B1 card. These
copies retain only fetched pages, not all selected tasks. Identity decorations
may refresh under their separate fence. Local optimistic mutation overlays stay
immediate but mark the affected generation stale/pending; its exact counters
remain historical B1 facts, not adjusted claims about the optimistic selection.
A new filter with no complete generation stays pending, and any displayed prior
selection remains tagged with its actual identity. Eviction of retained cards
may leave historical metadata readable; it does not authorize basis mixing. If completed membership becomes
stale before first card fetch, retain it only as cached historical metadata,
never combine it with current task fields as if coherent. A later implementation
could copy bounded cards into immutable output, but its extra O(M) bytes and
identity/presentation semantics need separate review.

Current query basis covers task selection facts, not all presentation. Identity
refresh/owner refresh does not advance it; summaries overlay live display names
and workspace availability. Frozen K/W/coordinator never comes from that
presentation. Label identities as presentation revisions independent of exact
membership authority; do not call these APIs an immutable full-row snapshot.
Decoration refresh is a separate bounded identity read/cache write tagged with
its own owner/entity revision and carrier/session epoch. A delayed decoration
reply cannot overwrite a newer directory revision or change K/W/coordinator.
If the existing source has no comparable decoration revision, fence by the
per-identity local request/mutation epoch and replace the whole bounded
presentation record; never claim it was captured at task basis B. Unknown or
missing decoration keeps the exact actor ID and cached presentation. The
implementation must fixture owner moves, rename/remove and stale responses.
If product requires all row decoration at B, retained payloads/revisions are an
additional architecture choice. Project-ID mapping is bound during admission,
not reinterpreted from changed live project metadata.

Cancellation/expiry mark an indexed job terminal in O(1). Maintenance selects
<=16 due jobs by indexed deadline and admits at most one GC quantum per worker
round. Delete <=64 rows from one owned table using an indexed keyset and point
DELETEs; no FK cascades, whole-build DELETE, or DROP TABLE for a large job.
Cleanup includes memberships, status rows, staged values, chunk receipts and
metadata, and only releases referenced sealed sets after their last reference.
Reference counts change at admission/terminal cleanup in O(1); no consumer scan.
Storage remains charged until each bounded deletion commits. SQLite free pages
and WAL/checkpoint space are physical costs, not freed merely by logical row
counts. Use existing bounded storage maintenance for page reclamation; no VACUUM
in request/job finalization. Crash resumes checkpointed work/cleanup only if its
basis still matches, otherwise marks restart_required. A restart does not make
an incomplete input seal complete.

## Budgets and admission candidates

These numerical settings must be checked against the measured tables in
EVIDENCE.md. They are conservative experiment settings, **not guaranteed
production limits**. SQLite progress callbacks every K steps do not by themselves prove a total
quantum instruction bound across multiple prepared statements: each statement
can have an unreported residual. The prototype counts with stride 1; a production
implementation needs per-statement SQLITE_STMTSTATUS_VM_STEP accounting and a
conservative interrupt residual (or equivalent exact accounting). Its timing
overhead differs from the Rust bridge.

A hard wall deadline cannot be proven by Python callbacks
or SQLite VM instructions; OS scheduling, page faults, B-tree depth, commit/fsync
and busy waits remain outside the instruction counter. Bridge admission/queue
latency, existing <=100 ms read rule and producer contention need release tests.

| Phase/resource | Candidate bound | Enforcement needed before production |
| --- | --- | --- |
| Header work | <=64 admitted headers per quantum | Indexed project+number seek; compact <=2 KiB per header |
| Relations | <=64 facts, <=1024 indexed probes per quantum | Independent fact cursor; byte-checked fact <=128B plus keys |
| Worker logical bytes | <=512 KiB logical read, <=256 KiB logical write/quantum | Charge headers, relations, membership, metadata and index amplification separately; logical bytes are not disk IO |
| SQL VM | <=100,000 instructions/quantum | Research callback every VM step; production must account per-statement VM steps plus interruption residuals, whole quantum rollback; fixed SQL only |
| SQL elapsed target | 20 ms worker guard, <=100 ms public read | Soft callback deadline only; zero busy wait, reschedule on busy; actual end-to-end gate still required |
| Metadata output | <=100 records and <=64 KiB/page | Account actual encoded bytes including status and cursor |
| Summary output | <=500 and <=1 MiB/page, <=64 KiB row+identity | Existing bounded summary contract plus live-basis validation |
| Concurrency | 1 active writer quantum globally, <=4 admitted builds/device, <=2/project, <=16 global jobs | O(1) charged reservations; foreground writer priority and round-robin jobs |
| Storage | 64 MiB logical/build, 16 MiB/set, 256 MiB global selection quota | Charge inserts/index allowance before mutation; global physical DB/WAL headroom separately reserved |
| Lifetime | 60s incomplete input idle, 120s build attempt, 300s completed lease | Indexed expiry; scheduling deadline ends attempt, not a completion guarantee |
| Automatic restart | <=2 restarts after initial attempt, <=120s total operation budget | Jittered 250/1000ms only within budget; user retry afterwards; no client automatic drain |
| Scheduling | <=1 quantum per job per round, <=10 worker quanta/sec globally initially | Foreground backpressure; progress latency reported, not hidden |

All counts have checked integer arithmetic and supported task-number domain
SQLite i64/JS-safe representation explicitly encoded on wire (decimal string if
outside client safe integer range). Oversized fixed fields refuse in admission;
large but valid *sets* paginate. Quotas may make otherwise legitimate scopes
unavailable. They are capacity decisions, not a narrowed success definition.
Physical disk reserve, fair queue behavior, worker duty cycle and global quota
numbers remain unvalidated by the research model; do not treat this table as
shipping readiness. Large per-quantum VM or index cost should lower row budgets,
not increase the <=100 ms requirement.

## Progress basis and mutation gate

Today's `task_query_generation` is store-wide, advanced on every task query
refresh/remove even when values compare equal. Any unrelated project's history,
read mark, dependency or metadata update can invalidate an in-flight build.
The prototype compares global basis with one project epoch using deterministic
mutation schedules. A project epoch is a narrowed invalidation scope, not a
journal, change log, retained snapshot or guaranteed semantic-diff detector.

A production project epoch must update transactionally on create/delete/move
(old and new projects), task state/status/labels/assignment/priority, attention
(read/watch/dismiss/history append/reparent/repair), dependency endpoint changes,
deferred-delivery metadata and projection rebuild/repair. Because card payloads
are loaded under this same proposed task basis, also advance on every
non-decoration summary mutation: title, creator/retained actor references,
dates, attachment counts, links/trackers and review summary metadata. A title-only
edit must invalidate an unfinished build and fence a completed page. This
conservatively scopes by project, not just predicate-value equality. Pure frozen-roster
presentation updates need not invalidate selection. No eager query enumeration:
one epoch update per affected project, independent of admitted selection count.
Both old/new task/project epochs must advance on cross-owner moves. Incarnation
rotates on incompatible rebuild/replacement; all affected jobs refuse/restart.
Startup can invalidate old jobs in bounded maintenance rather than scan them
synchronously. Adding a per-project epoch is a coordinated Store/projection
change with #901, after approval, not a prototype production patch.

Quiet interval length must cover staging-independent evaluation + publication
+ needed card reads. Bounded total attempts cannot guarantee such an interval.
If each quantum takes service interval d and build needs T quanta, the exposure
window is approximately T*d plus queueing (not just summed SQL CPU). With writes
at rate lambda and a Poisson approximation, a quiet interval probability is
exp(-lambda*T*d); this is illustrative, not a measured arrival model. The
measured deterministic schedules and restart counts are the acceptance gate.
If no stable result is reached within the admitted attempt/restart budget,
return unavailable/restart_required and retain last complete cache at its real
basis. Never claim bounded conversion is complete merely because each failed
quantum was bounded.

## Total work, costs, alternatives and unresolved gates

Let N be candidate headers, L inspected relation facts, M selected memberships,
D selected distinct statuses, R staged roster/filter/config members and J active
selections. Evaluation is O(N+L) admitted records plus O(R) input. Indexed probes
and incremental index updates add tree-depth/write cost; this is not a claim of
linear CPU or constant IO. Storage is O(M+D+R), with several indexes per record,
plus input chunk receipts and transient WAL. Per request boundedness does not
make total work, first-result latency, physical bytes or write contention free.
Each retry can repeat the whole scan. Low selectivity can return a small total
only after inspecting the full canonical candidate range.

| Alternative | Exactness/progress | Cost/decision |
| --- | --- | --- |
| Existing project/agent additive buckets | Exact for their existing fixed questions only | Cannot derive arbitrary overlapping selected groups/status discovery |
| Direct COUNT/GROUP BY/NOT EXISTS query | Exact after potentially whole selection scan | Small response/LIMIT does not bound VM, sort or lock time; rejected |
| Client drain/group all pages | Exact only after full coherent catalogue | Violates read-on-open/bounded-client requirement; rejected |
| Eager per-selection producer maintenance | Can reduce read latency if complete | O(J) predicate/fanout per mutation, roster changes can reclassify O(N); not authorized |
| Global-basis resumable build | Exact only during a quiet whole-store interval | Prototype restart/starvation comparison; unrelated projects can deny progress |
| Project-basis resumable build | Exact during a quiet selected-project interval | Preferred investigation; new producer epoch hooks/index cost; still no churn guarantee |
| Retained snapshots/versioned facts or replay | Could provide progress across mutation | Additional retention/WAL/version/GC architecture under #892; no scope expansion here |
| Pending/refusal with old exact cache | Honest finite resource policy | Needs explicit activation/product decision; no exact answer for a new unavailable scope |

Required acceptance fixtures and prototype coverage live with the tests and
EVIDENCE.md. Production acceptance additionally requires canonical-versus-
projection parity (including every invalid off-page field), producer transaction
and rollback coverage, configured empty-column/stray pagination, card basis
race, cache transaction/local mutation fences, ownership/wire/capability fixtures,
physical storage/WAL and migration/producer costs, reader/writer fair contention,
small/large accounts and controlled quiet/normal/sustained mutation measurements.

No blanket cost acceptance is inherited from #890/#900. Measured regressions
must be shown to the lead/user as the specific delta. This design's review can
complete while production remains unauthorized and #891 List/Board conversion
remains incomplete. A claim of production-ready bounded conversion requires
all of those later gates, including usable progress or an explicitly accepted
support boundary.
