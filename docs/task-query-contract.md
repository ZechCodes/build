# Bounded task consumer queries

Wire 3.27.0 extends the opt-in summary reads from
[bounded-task-reads.md](bounded-task-reads.md). The legacy catalogue, detail,
chooser search, and write responses keep their contracts. There is no search
capability in this slice; [task-search-options.md](task-search-options.md)
records the unresolved matching and resource contract.

## Independent capabilities

Clients check the live greeting before dispatch, and persist support separately
from cached content. `tasks.listSummaries` alone does not permit new selectors.
All selectors below require `tasks.list {summary:true}`; using them in legacy
mode or passing null is invalid. The existing scalar/plural selections remain
OR within a dimension and AND across dimensions. New predicates are ANDed.

| Capability | New selectors or verb |
| --- | --- |
| `tasks.counts` | Materialized project and explicitly requested agent buckets |
| `tasks.lookup` | Project-number point lookup |
| `tasks.listWatched` | `watched:boolean` |
| `tasks.listAttention` | `attention_rule:"asked"\|"legacy"` |
| `tasks.listLinkedWorkspace` | `linked_workspace_id:string` |
| `tasks.listTracked` | `tracker_agent_id:string` |
| `tasks.listReviewMode` | `exclude_review_mode:"pull_request"\|"snapshot"` |
| `tasks.listOrder` | `order:"number"\|"updated"\|"priority"\|"anchor"`, `done_since:integer` |
| `tasks.listReadiness` | `unfinished`, `blocked`, `held`, `undelivered` booleans |
| `tasks.listExcludeAssignee` | `exclude_assignee`: exact tagged assignee object |

Opaque relation/agent IDs are exact, nonempty, and at most 128 UTF-8 bytes.
Review mode defaults to snapshot for an existing historical review header that
omits its mode; an absent review is distinct. Pull-request summary metadata
includes `review_summary.mode`. Exact canonical workspace and tracker relations
are stored independently of their capped display arrays. A returned row's
`matched_memberships` records the query's relation match; clients retain it with
that query membership, rather than overwrite global task facts with it.

`unfinished` means state is not Closed and workflow status is not Done.
`blocked` follows the canonical dependency projection. `undelivered` covers
waiting, queued, provisioning and failed deferred assignment states. `held`
means an assignee exists, or an undelivered target exists. These are static
facts; no persisted working task status is introduced.

## Counts and lookup

`tasks.counts {project_id,agent_ids?:string[]}` accepts at most 100 entries,
rejects invalid IDs, and deduplicates/sorts valid agent IDs. It returns
`{project_id,unfinished_open_count,watched_unread_total,agents,basis}`. Every
agent entry has `agent_id` and the same two count fields. Unknown agents have
explicit zero buckets. The project bucket includes every unfinished task;
an agent bucket includes only tasks whose exact assignee is that agent.
Deferred assignment targets and legacy project-agent assignments do not acquire
an arbitrary agent bucket. Unread totals sum watched unread entries on unfinished
tasks, rather than count tasks with unread entries. There are at most 101
materialized bucket point reads and 65,536 serialized result bytes. No request
counts or sums a project catalogue, roster, canonical body, or timeline.

`tasks.lookup {project_id,number}` accepts a positive task number representable
by the store. Its result is `{project_id,number,task,identities,basis}`. `task`
is one bounded summary or explicit null for a confirmed miss; identities are
shared and bounded as in summary pages. The complete response is at most
65,536 bytes. An unknown project is an error, rather than a task miss.

## Ordering and static streams

Number is descending and remains the default. Updated orders by supported update
milliseconds descending, then number descending; a genuinely missing timestamp
has recency rank zero. Priority orders urgent, high, medium, low, none,
then number descending. This preserves the stable priority pass over the
existing number-descending catalogue. Anchor orders the summary's activity
anchor ascending, then number descending, with unknown anchors last. Anchor pages return `order_anchor_ms` with the exact
ordering value (explicit null when unknown); retain it as query membership
metadata. A valid safe-integer session pair uses `session_started_ms`; otherwise
use parsed `created_at`, then parsed `updated_at`, then unknown. This preserves
the summary-only Inbox fallback. The internal unknown sentinel never crosses
the wire as a timestamp.

Ordered timestamps use the separate
[`ecmascript-iso-v1` domain](task-query-timestamp-domain.md): complete Gregorian
ISO dates (UTC), or full uppercase `T` date-times with explicit `Z`/numeric
offset. Native fractions accept 1–9 digits, truncating/padding the wall-clock
fraction to milliseconds before offset conversion. Expanded years and the
inclusive ECMAScript TimeClip range are supported. Zone-less date-times,
lowercase forms, leap seconds, overflow dates, `24:00`, informal parsing and
other undeclared forms are unsupported. Missing/null/empty values are unknown,
not unsupported. The documented native fraction extension matches the tested
Node/Chromium engines; it is not an all-browser or arbitrary-Date.parse proof.
The legacy history parser and canonical imported bytes are preserved.

Before any successful affected page, at most two partial-index point seeks
check a conservative whole-project validity scope in the same read snapshot.
Updated ordering checks update timestamps; anchor ordering checks only the
fallback fields needed when a valid session pair is absent. An unsupported
creation fallback refuses anchor ordering even if update time is valid.
`done_since` checks Done timestamps for Done tasks. Invalid rows beyond the
candidate window or excluded by other selections still refuse this project/order
scope. Correction, reparenting, removal and rollback update validity atomically.
Number/priority reads without cutoff, counts and lookup remain independent.

Refusal is `error_code:"unavailable"`, `retryable:false`, with details
`{reason:"task_query_timestamp_unsupported",scope:"project_order",order,
project_id,domain:"ecmascript-iso-v1",field}`. The field is `updated_at`,
`anchor_fallback` or `done_at`. No result or success cursor is issued. Preserve
cached rows/cursors and suppress automatic retry until the data or chosen mode
changes. Explicit legacy compatibility does not become completed bounded
conversion. This gate intentionally avoids combinatorial validity indexes.

`done_since` is an inclusive epoch-millisecond cutoff and only admits Done
tasks with a parsed completion time at or after that cutoff. It does not move
with wall-clock time; the client owns its session cutoff and query identity.

For agent A, use priority order and separate streams:

| Group | Static selections |
| --- | --- |
| Working | Assignee A; open; non-review/non-Done columns; blocked false; undelivered false; enabled only while cached A is working |
| Assigned | Same active assignee/columns; blocked false; while working restrict undelivered true, otherwise omit that restriction |
| Blocked | Active assignee/columns; blocked true |
| Tracked | Tracker A; exclude exact assignee A; all states/statuses |
| In review | Assignee A; open; In review |
| Done | Assignee A; open; Done |
| Closed | Assignee A; closed; any column |

Configured column vocabulary supplies the full active-column selection; loaded
rows do not define it. These groups are disjoint. Assignment takes precedence
over tracking; Closed precedes the history column decision. The client
concatenates in the table's order with independent continuations. Cached
working/coordinator changes retarget the relevant memberships and fence older
replies. Legacy readers keep their historical recency behavior.

Modern attention selects unfinished user-assigned unblocked tasks or watched
unread asks after the read marker. Legacy attention additionally includes
In-review tasks, and its unread-comment branch respects watched inbox existence,
positive unread and dismissal state. Compact meaningful-activity facts preserve
the inbox's bookkeeping rules. This remains independent from assigned agent
placement and cached runtime working/coordinator observations.

A workspace overview unions exact linkage with existing assigned-agent queries
under updated order. It does not replace the assigned-only Tasks predicate.
The client merges ordered heads and deduplicates IDs. An empty sparse page with
continuation supplies no head: output that depends on it remains incomplete
until explicit continuation obtains a head or proves exhaustion. Do not display
another branch's candidate as globally next or drain pages automatically to
fill a page. Watched cross-project Inbox ties retain source-project order,
then each project's number order; `inboxOrdering.byAnchor` compares anchors only.

## Basis, continuation and write fences

`basis` is an opaque store-incarnation fingerprint and persisted store-wide
projection generation, read with rows/buckets in one transaction. Equal tokens
mean the same projection generation in the same store. Clients must not parse,
increment, lexically order, or compare generations from different stores.
Ordinary reopening preserves the token until a producer changes facts; replacing
the store or rebuilding affected projections can invalidate it. It is not a
journal cursor, acknowledgement of a write, or retained snapshot lease.

Existing write replies do not return this generation. Clients capture their
local mutation epoch when issuing reads, reject replies issued before a pending
read/watch/assignment mutation, and refresh after the write acknowledgement.
They install authoritative returned totals directly, without arithmetic against
older totals or write envelopes. A late read cannot erase immediate local user
effects. The generation provides consistency across projection reads; local
request/write fences provide mutation ordering.

Cursors authenticate store/project, normalized selections, order and seek
position. A cursor additionally carries its traversal basis. A changed basis
returns `error_code:"conflict"`, `retryable:false`, and
`details:{reason:"task_query_basis_changed",restart:true}`, without result or
replacement continuation. A changed selection/foreign/tampered cursor is invalid
parameters. Keep cached content while refreshing, but begin an explicit new
first-page generation; do not loop retries or append old pages to a new basis.
Unrelated store-wide projection writes may conservatively require a restart.

Summary limits remain default 100/max 500, 1 MiB response, 64 KiB row plus
identity contribution and at most `4*limit+1` candidates in chunks up to 32.
Ordering uses indexed ranges; predicates use bounded point facts and exact
relation seeks per candidate. Sparse empty pages with continuation are valid.
Total traversal may cover the project through explicit continuation; no request
or background bootstrap silently exhausts it.

## Producer and repair ownership

Supported Rust producers update static facts, exact relations and old/new
aggregate contributions in the canonical transaction. Task writes own typed
relation replacement; history changes use bounded retained summaries without
decoding task bodies or relations. Dependency changes refresh affected tasks;
review/deferred metadata producers refresh their static facts. Reassignment,
reparenting, deletion and import/repair include both old and new contributions.
Failure rolls back facts, counts and generation together. Touched zero buckets
are retired. Count correctness has no time-relative periodic sweep.

Startup fingerprint/manifest ownership includes all new tables, indexes and
producer definitions. Existing resumable bounded backfill/reset machinery runs
before the eight-reader pool is admitted; intact reopening skips rebuilding.
Independent SQL has no automatic maintenance guarantee and must use explicit
repair. Relation write work scales with canonical relation cardinality; migration
still must parse large imported records. These costs require separate startup,
write, memory/storage and reader-contention evidence.

Shared witnesses are `fixtures/api/v1/tasks.list.json`, `tasks.counts.json` and
`tasks.lookup.json`. They preserve the historical legacy examples. Scope review
of this design is preliminary; final independent scope/code and Astra reviews
must inspect the exact reviewed implementation head.
