# Task #891 ordered stream composition

Decision input for #900/#891. These are required predicates, not speculative
RPC selector spellings. Exact requests and fixtures belong to #900. Preserve
existing global grouping/order; never sort an arbitrary loaded number page and
call it the first globally ordered page. No new search request is authorized.

## Agent task surfaces

The source is `trackerAgentTasks.js`: `surfaceStateOf`, `historyStateOf`,
`surfaceEntryOrder` and `SURFACE_STATE_ORDER`. Modern summaries always carry
readiness. `working` is a cached per-agent observation; the server must not
invent a task working status from it. Existing unsupported/legacy readers keep
their historical recency path.

For one agent A, define exact assigned membership `assignee == agent:A`, and
exact tracked membership from the canonical tracker relation. Assignment wins
over tracking, including finished tasks. Required streams are:

| Display group | Exact static membership |
| --- | --- |
| Working | Assigned to A, open, status neither In review nor Done, unblocked, no undelivered assignment; enabled only when cached A is working. |
| Assigned | Assigned to A, open, status neither In review nor Done, unblocked; when cached A is working narrow to undelivered assignments. |
| Blocked | Assigned to A, open, status neither In review nor Done, blocked. |
| Tracked | Canonically tracked by A AND not assigned to A, across all states/statuses. |
| In review | Assigned to A, open, status In review. |
| Done | Assigned to A, open, status Done. |
| Closed | Assigned to A, closed, any status. |

These streams are disjoint. Closed wins over Done/In review. Static blocked
wins over dynamic working. Undelivered states are waiting/queued/provisioning/
failed, matching `taskHasUndeliveredAssignment`; they cannot be Working.
Modern streams sort priority descending then task number descending, matching
the existing stable priority pass over the canonical number-descending source.
An ID tie-break is defensive; task numbers are unique inside the project.

The client concatenates groups in the existing order. It opens one bounded
page per visible group, with independent continuation per group. A later group
can be displayed beneath a prior group's explicit continuation; it is never
used to fill a prior group's missing first rows. A single flat pager would
instead need to finish the earlier stream through explicit user demand before
advancing groups. Neither design auto-drains streams. Overlap protection by
task ID is defensive, not a substitute for exact exclusions in the server
query: filtering an entire tracked page of assigned duplicates can otherwise
produce no useful first page and invite hidden traversal.

A cached working transition retargets Working/Assigned streams as a unit;
their active generation changes and old replies cannot append. A roster or
coordinator change uses the same fence. A task mutation/push marks affected
loaded memberships stale and refreshes their first pages rather than merging
old continuations across a changed projection basis. The upcoming #900
generation-bound restart contract must be applied before enabling these paths.
It is acceptable to retain cached content during refresh; it is not acceptable
to label incompatible old membership as current or turn unknown into empty.

## Other overlaps and ordering

- Workspace overview is the union of exact linked-workspace membership and
  exact assigned-to-workspace-agents membership. Unlike the assigned-only
  workspace Tasks tab, these branches can overlap. To provide an exact first
  updated-at/number page without draining, each branch must supply a bounded
  ordered head. Perform an ordered merge, deduplicate by task ID and retain
  each branch cursor/head; request a next branch page only when explicit user
  continuation needs it. If a single visible page requires a strict request
  bound even under complete overlap, prefer a server exact union selector or
  server-side exclusion primitive; no finite number of filtered pages alone
  guarantees a full deduplicated page. Sparse results retain continuation.
  An empty sparse page with continuation supplies no next-item ordering bound.
  Do not emit an item from any other branch until every contributing branch
  has a known head, authoritative frontier proving its ordering, or exhaustion.
  If the admitted request budget runs out first, preserve an incomplete
  continuation; do not describe the partial output as a full ordered page.
- Dashboard Needs you may overlap Active. Preserve that intentional overlap
  across distinct sections; deduplicate only within a section. Dynamic
  running/coordinator facts remain cached client inputs. Static group and
  attention streams need the exact source predicates, with separate cursors.
  Modern Done uses done-at cutoff and existing time grouping; its server order
  must produce that group's true head. Legacy attention's inbox dismissal gate
  stays a client predicate until its precise bounded contract is settled.
- Watched Inbox requires the global activity anchor order, not task-number
  order. Per-project streams can be merged by the existing `byAnchor` tie-breaks
  from `inboxOrder.js`; counts never derive from the loaded heads. A cross-project
  merge retains each project's independent head/cursor and does not exhaust
  projects just to fill a menu or resolve a reference.
  Admission must also cap project-head requests per explicit operation; opening
  the view must not introduce an all-project sweep into #891 bootstrap. Unknown
  project heads obey the same merge barrier as empty sparse branches.

All selection, order, locale and exclusion facts that affect server membership
or cursor binding must be present in canonical cache query identity. Cached
working/coordinator facts that retarget static streams also belong to the
controller generation. New selector dispatch waits for independent named
capabilities and pinned fixtures; #891 does not change its approved base here.
