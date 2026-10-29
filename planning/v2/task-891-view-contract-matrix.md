# Task #891: exact List and Board contract prerequisite

Design draft for lead review, 2026-10-29. This document changes no implementation
or wire contract. The delivery decision is a **new, separately owned backend
prerequisite under #887**, followed by #891 integration. It does not reopen #900,
assign this work to #901, or authorize first-sync, journal, search, or detail
API work. Proposed operations below are concepts; their RPC and capability
names are deliberately unassigned. The backend owner must settle and publish
them with fixtures before consumer dispatch exists.

The source audit used the shared checkout at `2659e413`, including
`docs/task-query-contract.md` and the actual `trackerTasksPane`,
`trackerTasksBody`, `trackerAttentionModel`, `trackerBoardModel`,
`trackerBoardRender`, `trackerFilters`, `trackerTaskReadiness`,
`projectRosterFeed`, and `workspaceTasksTab` modules. Concurrent #891 cache
foundation changes do not establish these missing contracts. This narrower
audit supersedes the earlier contract-gap document's suggestion that loaded
counts might suffice for these views. Exact counts and existing global ordering
are required; changing to ungrouped pages or approximate counts is unauthorized.

## Exact selection and placement

Let `F` be the selected project tasks, after all user filters and any workspace
scope. `trackerFilters.matches` ORs choices within each of state, status, label,
and assignee, then ANDs dimensions. `any` assignee means held, including an
undelivered target; exact agent selection refers only to the actual assignee.
An empty intersection with a workspace's agent set is an exact empty selection,
never an omitted restriction. On Board, `shownFilters` deliberately removes
the status filter; the other dimensions still apply. Filters and configured
columns have distinct identities and lifecycles.

Let `U` mean state is not Closed and status is not Done. Let `K` be exact known
holder membership from the **cached** project roster interpretation below, and
`A` the selected cached Needs-you rule's canonical attention predicate. List is
the following stable partition of the number-descending `F`, in this order:

1. `G1 = F ∩ U ∩ K`: “In progress with an agent”. Working/idle, blocked, and
   deferred-delivery state do not alter this placement. This is not the agent
   surface's runtime Working group.
2. `G2 = (F ∩ A) \ G1`: “Needs you”. Attention never overrides known-holder
   placement; an overlapping task occurs exactly once.
3. `G3 = F \ (G1 ∪ G2)`: “Other tasks”, including finished tasks when selected.

`A` already implies unfinished. The asked rule includes unblocked user-assigned
tasks or watched unread asks after the read marker. The legacy rule additionally
includes In review and preserves its inbox existence/unread/dismissal gate.
The current model can consult cached details/inbox rows; bounded conversion must
use #900's canonical attention projection with the same rule, preserving newer
local read/watch effects and never obtaining membership by fetching every detail.

`groupedRows` applies one global `visibleCount` budget, initially 25, to G1 then
G2 then G3. Collapsing a group hides its rows without redistributing that budget.
`paintGroupedTaskRows` renders exact group counts and “Showing X of total”, where
total is `|F|`, independent of loaded rows. A list page must therefore be a prefix
of this concatenation, not three independently sorted fragments of an arbitrary
project page. If the next G1 head is unknown, a G2 row cannot fill its slot.

Board uses configured columns in configuration order, including empty columns.
Unconfigured statuses occurring in `F` follow them in first-occurrence order in
the number-descending input: descending maximum selected task number per stray
status. Every column count is its exact selected count. Normal columns retain
number-descending task order. Backlog switches to nonempty Blocked then Ready
groups if **any selected Backlog task** satisfies `taskHasReadiness` (dependencies
or deferred-assignment object present); each group has stable priority order,
then original number order. Blocked is `dependencies.blocked === true`; Ready
is its complement. Presence of readiness metadata and positive blocked state
are different facts. Modern summary readiness presence must be reconciled with
this activation rule explicitly, not inferred from one loaded task. Unknown
columns do not gain Backlog grouping. Current Board renders all selected cards;
bounded continuation must visibly preserve completeness/remaining-work state
and exact order, not silently present a page as the whole column.
Status discovery must retain canonical status identity. A truncated display
sentinel must not merge distinct stray statuses or manufacture a configured
status; an unsupported canonical value requires an explicit domain refusal.

## Consumer → missing-contract matrix

The four products below are independent: a global selected total does not
provide disjoint group totals; group totals do not provide ordered heads; heads
do not discover every column. A producer may share their implementation, but
must not substitute one for another.

| Actual consumer and source | Existing APIs without catalogue draining | Required request/response/capability concept | Work and ownership requirement |
| --- | --- | --- | --- |
| List footer: `paintContext.paging.total = state.shown.length`; `trackerTasksBody.paintGroupedTaskRows` uses it for exact “Showing X of total” and empty state. | **No.** `tasks.counts` only returns project/agent unfinished counts and watched-unread sums, not arbitrary `F`; summing label buckets double-counts overlaps. | Exact selected-total product, keyed by normalized filters, workspace scope, roster identity where relevant, and basis. Return a count only when complete; otherwise explicit pending/refused state. | One published scalar is a small read, but obtaining it may require all candidates. Materialize an admitted selection or resume bounded evaluation as described below. Never `COUNT(*)` over an unbounded runtime selection and call its one-row result bounded. |
| List three headings: `attentionGroups` → `groupedRows.count`. | **No.** Project/agent unfinished buckets do not intersect arbitrary filters/attention. Summing exact agent buckets cannot count filtered Needs-you complements or Rest. | Three exact, disjoint group counts under one selection/roster/rule/basis, with `sum = selected_total`. | Compute each selected candidate's single placement once. A readiness/attention/assignment producer updates facts; an on-demand selection build owns grouping. Publishing partial counters as totals is forbidden. |
| List first 25 / Load more: stable partition before slicing. | **Partly.** `tasks.list` supports positive attention, plural assignees, unfinished, number order, and **one** exact `exclude_assignee`. It lacks the full known-holder exclusion and negative attention/rest expression. Grouping one loaded page loses older G1 tasks to newer G2/G3 tasks. | Ordered membership/head product for each exact disjoint group, with independent opaque continuation and completion, or a single ordered stream with `(group rank, number descending)` order. Same query identity as totals. | An indexed selected materialization supplies bounded head seeks. A sparse primitive stream may return no head with continuation; retain the barrier instead of silently advancing. One global pager retains current prefix/collapse semantics. |
| Board column headings: `trackerBoardRender.columnHtml` currently uses `column.tasks.length`. | **No.** Neither configured columns nor `tasks.counts` supplies counts under Board's `F`. | Exact selected count per requested/discovered column, pinned to Board selection/basis; explicit zero for configured empty columns. | Counts belong to the same completed selection as column membership. Request/response pages bound number and bytes of column records; avoid an unbounded map of statuses. |
| Board column discovery/order: `trackerBoardModel.boardColumns` appends `strays`. | **No.** `tasks.columns` supplies configuration. `tasks.filterChoices` supplies independently paged project-wide status choices, not statuses present under Board filters or their first-occurrence order. Loading only task heads can miss whole columns. | Filtered distinct-status discovery with count, selected maximum number/order key, configured/unknown identity, continuation, and explicit discovery completeness. Configuration is a separately cached input. | A selection build incrementally records status counts/maxima; indexed output orders unknown statuses by maximum selected number. Distinct-status cardinality is unbounded relative to one response and may approach task cardinality. No fixed first-100/500 truncation. |
| Board cards / Backlog: number order; Blocked then Ready stable priority order when readiness exists. | **Partly.** Exact status, `blocked` and `order:"priority"` exist. `blocked:false` is the Ready predicate. These do not provide whole-column counts or prove the whole-column readiness activation condition. | Column/group ordered membership and bounded continuation, plus exact `has_readiness` over selected Backlog (or an explicitly reviewed equivalent for always-present modern readiness). Group presence follows exact nonzero membership. | Reuse existing bounded summaries for payloads at the matching basis. Index `(selection, column, subgroup, priority rank, number)` as needed. Preserve priority fallback semantics; no full-column client sort/drain. |
| Workspace List/Board: `workspaceTasksTab.heldHere` and `state.only`. | **Partly.** Plural assignees express at most 100 exact actors per request; this is not an unlimited roster or arbitrary extra-assignee-filter intersection. | Exact workspace roster set as a separate scope input; AND it with user assignee/other filters before totals, partitioning, and column discovery. Support large sets through a completed staged set/reference, not dropped actors. | Cached workspace residence owns membership. This is assigned-only: no linked workspace, tracked task, user, or legacy project-agent expansion. A bounded set page does not prove set completion. |
| Filter menus: `filterOptions` / `labelsOf` / `assigneeFilterOptions`. | **Yes for the existing independent choice pages**, with cached roster labels and configured status vocabulary. Canonical #900 choice-size correction must be integrated. | No additional query verb required for this purpose. Keep choice cursors/completeness independent of selected counts and visible rows. | Existing `tasks.filterChoices` indexed `limit+1` distinct-choice reads and byte caps apply. Menus must not drain tasks, and choice absence on one page is not proof of no such actor/status. |

The existing summary request limits remain default 100/max 500, at most
`4*limit+1` candidates in chunks of 32, at most 1 MiB result, and at most 64 KiB
per row plus identity contribution. Existing counts accept at most 100 agent
entries, read at most 101 materialized buckets, and return at most 64 KiB.
These bounds describe **those APIs only**. They are not evidence that an
arbitrary selected count, SQL anti-join, large roster union, or grouped query
has bounded work. A SQL `LIMIT` after aggregation/sorting likewise proves no
candidate-work bound. B-tree seeks also have index-depth cost; “bounded” here
means bounded admitted records/bytes/steps, not an asymptotically constant CPU
instruction count independent of database size.

## Cached roster and generation ownership

`knownAgentsOf` uses every matching-project `projectRosterRows` entry's exact
agent IDs, deduplicated. A complete `project.projectRoster` replaces the legacy
owner row while workspace rows retain their own residents. An explicit agent
assignee absent from that set is unknown; “has some agent assignment” is not K.
Unknown agents still belong in `F`, and may go to Needs you or Other tasks.

For `project_agent`, `projectAgentOf` first finds the project's `entity_id` or
`run_id` owner row. With a complete project roster, only its persisted
`pinned_agent_id` can select the coordinator; a missing pin target means no known
holder. The legacy owner-row interpretation uses the working agent, otherwise
the first agent. Only that legacy interpretation depends on working/order.
An unfinished `project_agent` task enters G1 iff this resolution succeeds.
It must not be rewritten into an arbitrary exact-agent bucket. Idle known agents
remain G1. No server inference from a newer current roster may silently replace
the cached roster against which the client is rendering.

The new prerequisite must carry an exact immutable roster-input identity for
K, workspace membership, coordinator resolution and interpretation version.
Minimal viable transport is a staged, authenticated set built from the cached
input: bounded actor chunks, deduplication, declared membership count and a
completion seal; only the sealed set is usable. It can store a normalized
membership table without concatenating the entire roster into every request.
Owner/coordinator presence and legacy interpretation are explicit scalar inputs.
All real actors must survive staging. IDs have the existing exact supported
domain, with no lossy normalization. An alternative producer-issued immutable
roster token is viable only if its exact historical membership is retained and
available to the cache; that retention and update cost is additional work, not
an existing #900 guarantee.

A client hash alone is not a server membership set. The backend needs the
corresponding exact staged members or an authoritative retained version. Set
staging has O(R) total input/storage work for R roster members; chunks bound
each request, not the total. A large roster needs more chunks, never truncation.
Capacity refusal must leave this scope explicitly unavailable/pending with
cached content retained; it must not call a truncated set exact or empty.

Cache query identity includes device/store, project, normalized selection,
view semantics/order, attention rule, workspace set, roster/coordinator identity,
and relevant column/readiness interpretation. Changing any input fences old
responses and invalidates affected memberships/totals as a unit. A working
toggle alone does not retarget modern List K; a legacy coordinator resolution
change does. Group heads, counts, and discovery only compose at equal task
projection basis and equal roster/query identity. The store-wide #900 basis is
opaque and is neither a journal position nor a snapshot lease.

Local read/watch/assignment/move epochs fence replies issued before a pending
mutation. Publish only matching-generation records into cache; install exact
totals directly rather than adjusting old totals with new deltas. Keep prior
complete cached results readable during refresh, tagged with their actual
selection/basis/freshness. An old exact result cannot be relabeled as the new
filter's answer. A new selection with no complete cache is pending, not zero.
Rendering reads those cached facts/support states; live capability checks govern
dispatch only. A normalized row refresh cannot establish query membership on
its own. No connection-state branch decides whether a group exists.

## Fundamental limit and viable staged implementation

Exact arbitrary filter combinations have no general fixed-work request-time
answer in the current projection. Overlapping label unions need deduplication;
cross-dimension intersections and complements need joint membership information.
Precomputing single-dimension buckets does not supply it. Materializing every
possible selection/roster combination has combinatorial storage/update cost and
cannot be assumed. Adding a bounded actor list or returning only four integers
does not bound the SQL necessary to obtain those integers. An index helps seek
candidates; it does not make counting an arbitrary large selected range O(1).

Therefore these demands cannot all be met simultaneously: exact arbitrary
counts and grouping, bounded total work/storage/update cost independent of data
and active-query cardinality, immediate answers, and guaranteed completion under
continuous mutation. The viable compromise preserves **eventual exact results**
and **bounded work per turn/request**, exposes pending/refusal, and accepts
O(N) candidate evaluations plus O(M + D + R) storage in the conservative case (N
project candidates, M selected memberships, D distinct selected statuses, R
roster members). Indexed membership and accumulator maintenance adds tree-seek
and write cost; the candidate bound is not a linear total-CPU claim. It does not
guarantee an answer during perpetual churn. If
the lead requires immediate exact answers or guaranteed progress regardless of
churn as well, this remains an unsatisfied semantic/resource conflict and #891
cannot claim completion.

The smallest cohesive proposal is an explicitly admitted, resumable backend
selection build that publishes exact totals, partition memberships/order, and
column metadata together. This avoids separately inventing unrestricted Boolean
query algebra, filtered-count APIs, and filtered-distinct APIs. It still needs
independent capability negotiation and a documented primitive contract; this
document does not create a new RPC spelling.

1. **Admit exact inputs.** Normalize selectors and seal roster input before a
   build starts. Return an opaque build identity and pending state. Enforce
   per-device/project/global active-build and storage admission. Starting a
   selection is explicit query work, not a bootstrap/full-project sync side
   effect. Service ownership and request-driven versus scheduled advancement
   must be decided by the backend owner; a SPA cursor-exhaustion loop is not the
   implementation.
2. **Evaluate a quantum.** Capture basis B. Each short transaction first
   verifies B, reads at most Q keyset-ordered compact candidates, performs
   bounded source-fact and staged-roster point probes, updates private
   membership/count/status accumulators, and checkpoints in the same transaction.
   Reuse canonical bounded facts, not task bodies or timelines. Large filter
   inputs need staged exact sets too, or an explicit supported-domain refusal.
   Bound fact probes separately from task candidates: a task with arbitrarily
   many labels cannot be tested by an unbounded `EXISTS` join against the staged
   filter set. Seek its indexed label relations in fixed-size chunks, point-probe
   the staged selection for each, and checkpoint the unfinished candidate until
   its exact OR result is known. A positive match may short-circuit; absence
   requires exhausting that candidate's relevant facts through bounded worker
   turns. Apply the same rule to every multivalued intersection. Total work
   includes these relation facts, not merely N task headers. Do not run an
   arbitrary filtered aggregate and merely yield after it finishes.
3. **Bound all phases.** Backend review must pin numeric Q, maximum bytes read
   and written per quantum, membership/metadata row sizes, instruction or
   interruption budget, concurrent builds, per-operation request count, total
   storage quotas, and expiry. Starting candidate proposal: Q ≤ 128 task headers
   and at most 128 relation/set point probes per quantum, actor-input
   pages ≤ 100 IDs and ≤ 64 KiB, metadata result pages ≤ 100 records and ≤ 64 KiB;
   task output reuses #900's 500-row/1 MiB ceiling. These are proposed ceilings,
   not performance evidence. Compilation, joins, sorting, finalization, expiry,
   cancellation, and garbage collection also need bounded steps; no terminal
   whole-selection `GROUP BY`, index build, or unbounded `DELETE`. Maintain output
   indexes incrementally as rows arrive. Measure SQL VM steps/actual reader and
   writer contention as well as row counts. Oversize single facts refuse exactly
   rather than skip or spin.
4. **Publish only at completion.** A terminal transaction verifies B and marks
   the accumulated products complete atomically. Intermediate progress is not
   an exact total. The client may keep a previous complete display; a new query
   remains pending until the exact products are available. Published task pages
   seek materialized membership/order then load bounded summaries only if the
   live basis still matches. Metadata/column pages have independent continuation
   and explicit completeness; requesting one page never drains the remainder.
5. **Restart honestly.** With no retained snapshot lease or replay journal,
   any basis change between quanta or before publication invalidates the build.
   Return a distinct restart-required outcome; reject stale continuation, keep
   last good cached content, and begin only an explicitly scheduled fresh
   generation. Store replacement/repair, expired jobs and mismatched roster
   seals are similarly explicit. Bound automatic restart attempts and expose
   continued churn instead of retrying forever. A complete immutable cached
   result may remain visible as of B, but cannot supply new canonical row pages
   across a changed live basis. This design requires a sufficiently quiet build
   interval and guarantees neither such an interval nor unbounded job retention.
6. **Refuse without falsifying.** Oversized unsupported facts, admission/storage
   exhaustion, expired input sets and computational budgets produce structured
   pending/refused/restart states with retry policy and no success cursor/count.
   Admission caps concurrent work rather than silently capping real roster
   membership or stray-status cardinality. If a configured total storage quota
   cannot admit a legitimate large selection, report unavailable and retain the
   compatibility boundary; this is not completed bounded conversion for that
   domain. No fallback silently drains full client task pages.

This proposal deliberately spends total backend scan/storage work in a new
owned service. It needs a separate delivery decision approving that cost and
the pending/churn behavior. It is not a free consequence of #900, and it is not
authorized production work in this design-only task.

An eagerly maintained selection projection can later improve latency but moves
cost to producers. With J active arbitrary selections, one task mutation can
require O(J) predicate reevaluations and old/new bucket/index updates. Assignment,
state/status, labels, priority, history attention, watch/read/dismiss, dependencies,
deferred metadata, delete/reparent/import/repair all participate. A roster change
can reclassify O(N) tasks, or all tasks for changed actors; even one actor may
hold arbitrarily many tasks. A changed pin can affect all `project_agent` tasks.
Changing a common label can touch many active queries. Incremental staging can
bound each turn but leaves the replacement generation pending. Synchronous
atomic producer publication cannot promise constant update cost for these cases.
Startup/backfill/repair and teardown must be resumable and explicitly owned.
Choosing retained snapshots or mutation replay to guarantee progress would be
another architecture/scope decision, not permission hidden inside this plan.

## Representative acceptance fixtures

| Fixture | Required observable result |
| --- | --- |
| Older known-agent unfinished task plus 25 newer attention/rest tasks. | Older task heads G1 and consumes first visible slot; grouping only the first number page fails. Group counts cover all selected tasks. |
| Watched ask on known agent, same ask on unknown agent, user-assigned blocked task, Closed and open-Done tasks. | Known-agent ask occurs once in G1; unknown-agent ask can be G2; blocked user assignment alone is not attention; finished tasks are G3 when selected. Sum of disjoint groups equals selected total. |
| Idle known agent; departed/absent agent; valid persisted coordinator pin, missing pin target, no owner row, legacy working/first-agent fallback. | Exact cached ownership rules above; no arbitrary actor substitution, no modern working-state regroup, no project-agent assignment counted as workspace assignment. |
| Roster larger than 100 and tasks held only by the last staged member; duplicate residence; roster/pin changes mid-build. | No actor loss/double counting; incomplete set cannot publish; older roster-generation replies cannot overwrite new scope. Large complete sets retain exact semantics or explicit capacity refusal. |
| Multiple overlapping labels, several states/statuses, `any`/`none`/exact actors, workspace-assignee intersection empty/nonempty. | OR within and AND across dimensions, exactly one count per task; pending target only affects held/any, not exact actor assignment. Workspace membership stays separate from attention and linked-workspace membership. |
| Selected stray status exists only beyond loaded cards; project-wide status exists only outside current filter; >500 stray statuses; empty configured columns. | All and only selected strays are discoverable, ordered by selected maximum number; explicit discovery continuation/completeness; configured zeros remain. No project-wide choice list mistaken for filtered discovery. |
| Backlog blocked task with low number/high priority, higher-number ready task, off-page readiness-presence trigger, unknown priority and historical absent readiness. | Exact whole-column activation; Blocked then Ready, stable priority/number ordering; absence/presence behavior explicitly pinned for modern summaries. |
| Task mutation between build quanta, between terminal validation and next page, or after a local read/watch write begins; store rebuild. | No mixed-basis totals/heads; restart-required instead of stale append; local write epoch rejects earlier reply; cached exact prior generation remains distinguishable. |
| All candidates filtered out in one quantum; interrupted/restarted build; many concurrent filters; quota or unsupported fact. | Pending is not exact zero; checkpoint and facts commit/rollback together; no unbounded SQL finalization/fanout; explicit refusal preserves cache and never drains task pages. |
| Collapsed G1 and Load more; sparse/unknown G1 head; partial unknown-column metadata page. | Collapsing does not redistribute the global List budget; G2 cannot fill an unknown G1 slot; discovery incompleteness is visible rather than a claim of a complete Board. |

Implementation acceptance additionally requires measured large-N, large-roster,
high-overlap, high-distinct-status, and sustained-write workloads with explicit
reader SQL-step, producer/update, memory/storage and carrier-byte evidence.
These are future prerequisites, not tests run for this design document. The
delivery gate is: backend owner and pending/resource semantics assigned by #887;
wire/capability/fixtures reviewed and implemented; then #891 integrates against
that exact contract and verifies existing List/Board behavior. Until then these
consumers remain an explicit incomplete conversion, with no loaded-count or
ungrouped substitute presented as completion.
