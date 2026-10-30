# Task #891 ordinary-content authority

This client-only repair addresses an older-issued reply replacing newer cached
task bytes when their timestamps parse to the same browser millisecond. Native
timestamps ending in `.000900Z` and `.000100Z` demonstrate the problem. It does
not change timestamp parsing, RPC contracts, or the approved integration base
`fcec698761695148bcd8736e9bb516aefa00a8e0`.

## Authority and migration

Ordinary task headers and full bodies have separate private persisted
observation provenance. A producer captures its shared read ticket before the
actual RPC and annotates only the returned wire rows. Publication checks the
ticket and the existing session/context guards inside the cache transaction.
Receipt time, membership notes, optimistic rows, and copied catalogue rows do
not establish an observation.

The shared ticket allocates a persisted read number and captures local store
and project lifetimes plus a project retirement epoch atomically. These are
client cache generations, not backend revisions or projection bases. A blind
ordinary-content write or retirement advances the project epoch in the same
transaction. Replies issued before that write cannot restore retired proof,
including after an optimistic edit rolls back to identical bytes. Store reset
or scope eviction cannot recreate the old lifetime. Shared allocation must
remain monotonic across concurrent publishers and reopen; unsafe numeric
overflow refuses allocation.

An accepted component keeps its own dispatch order and timestamp. A strictly
newer existing parsed timestamp remains protected. A rejected reply cannot
stamp its receipt order onto retained bytes. Read/watch/dismiss markers, paired
unread counters, activity, PR summaries, and timeline/history retain their
separate reducers and authority. Recovering one family does not certify the
rest of a mixed row or establish absence/complete coverage.

Old records without genuine observation provenance remain unproven. An
addressed full revalidation may establish proof only while its captured
canonical record baseline and existing mutation guards still match. A summary
can establish header provenance, never body or history provenance. Unknown
content may remain painted while an admitted owner obtains a valid observation.

A genuinely issued raw observation can establish a new, absent header under
the captured domain fences. Existing unknown or retired headers are distinct
from absence, including same-task rows in the action transaction's already-read
replicas. Retained paint comes from an actual row; other rows provide separate
admission guards rather than a synthetic header assembled from their clocks,
timestamps, and bytes. This does not establish a new body or history observation.

An unchanged cached copy cannot lower a newer accepted component clock. Header
and body provenance are selected separately; body associations follow the body
source. Changed local copies retire affected authority. A full-task shape flag
does not supply observation authority: blind creation remains valid unproven
paint, while a refused raw observation cannot fall back to a blind write.

## Producer and writer boundaries

The repair covers the seven source groups approved on
#891/c/tc-01M654C4AF804R1AYMJ1ZWSBFN:

1. Shared ordinary-content helper, normalized summaries, and action replicas.
2. Legacy list/query/detail cache writers and paged list publication.
3. Sync/list producers and task-pane move, undo, and creation paths.
4. Mounted task-page and wanted-detail-feed full reads.
5. Workspace review-form full-task responses.
6. Optimistic checklist body changes and rollback.
7. Task-specific canonical publication and durable refresh adapters.

Optimistic and copying writers retire only affected component proof atomically;
untouched content retains its actual observation. A status move retires header
proof, while a checklist edit retires body proof. Canonical publication retains
its exact written-record, detail-basis, opening, and publication-version guards.
Header-only or uncertain-body acceptance cannot retire durable layers as a
successful full publication.

An admitted addressed full read updates its compact normalized observation and
detail in one transaction. The compact row is selected first from the raw
reply; accepted body provenance covers that actual selected header. Compact
rows never gain body or history proof. Session, owner and mutation-note guards
cover both outputs, and a quota refusal rolls both back.

Ordinary-content ticket or baseline rejection does not reject independently
admissible markers, watch facts or PR versions. Their existing reducers still
apply. A full read supplies no projection basis: changed compact facts clear
the retained basis and invalidate query freshness; identical compact facts may
retain their prior basis. Existing uncertainty stays uncertain, and missing
counter families remain unknown rather than becoming zero.

## Recovery bounds

Recovery belongs to existing mounted/admitted owners. The task page coalesces
one trailing read behind its active read. The wanted-detail feed retains its
four-read admission limit. Explicit uncertainty has a stable recovery identity;
identical metadata paints and a failed attempt do not create a retry loop.
Unmarked legacy records and summary-only rows do not start unopened-body reads.
There is no project sweep, new background service, or cursor drain.

## Watch extension under implementation

The separately authorized watch repair adds a watch-lineage observation and
retirement fence independent of ordinary header/body proof. Its source is the
actual pre-RPC ticket, not a list write number or receipt time. Watched state
and the exact presence/value of `watch_started_after` form one lineage;
read/dismiss markers, total unread, attention counters, activity, and PR facts
retain their own sources and floor checks. Neither a current watch observation
nor equal marker values certify the rest of a row or its projection basis.
Legacy and canonical marker comparator policies remain distinct.


An observed row keeps its own captured ticket through action, list, page landing,
and page folding. A writer must not reclassify an annotated raw row as a blind
local copy merely because no separate whole-record ticket was supplied. That
row ticket authorizes only eligible positive facts for the row. List omissions
require a separately admitted whole-list membership observation; row tickets
cannot prove deletion, exhaustive coverage, or absence. Retained page rows do
not acquire a new observation from being folded again.

Copies preserve the original source of unchanged family bytes. A changed blind
copy retires affected family authority rather than borrowing its copied proof.
The exact pending-owner stamp also survives replica/fold transport while the
owner remains unresolved; it is a protection against speculative replacement,
not a freshness clock. Rollback changes only its own watch paint and preserves
newer ordinary fields and list read-order high-water marks.

The shared pending protocol is still being implemented and reviewed. Its
required transaction contains a fixed-size exact-owner record, checked pending
count, watch epoch, and affected cache writes. Settlement removes only that
owner, once, in the same transaction as its count/fence changes and admissible
acknowledgment or rollback. Overlapping owners, replaced sessions, and stale
acknowledgments cannot settle another mutation. The local runtime pending guard
remains local protection; it does not prove cross-tab safety.

The bounded terminal-metadata amendment is approved for implementation, with
its combined proof still outstanding. Ordinary header selection stays separate
from a current-intent watch guard selected from an actual stamped replica.
Fixed per-task latest-intent metadata binds owner, dispatch order and cache
lifetimes; terminal disposition commits with exact-owner settlement. There is
no per-owner terminal history.

Existing admitted writers may consult that note in their publication
transaction to retire an exact matching terminal stamp or an older stamp
proven strictly superseded by the current same-lifetime intent. They must
preserve the newer pending protection. Retiring older protection never settles
that owner, changes its pending count, replays its rollback or certifies stale
paint. Missing, legacy and cross-lifetime bindings remain uncertain. Cleanup
of addressed replicas does not alone cover a copy racing address discovery;
independent cross-connection controls and lazy admitted-row cleanup are required.

Coordination uses a separate empty-entity namespace so ordinary project eviction
does not erase a still-live mutation. Missing or reset coordination is a sticky
unknown baseline. Beginning one new owner and settling it back to zero cannot
prove that no older owner was lost. An acknowledgment cannot bypass this
uncertainty. No timeout, cache reopen, or expired worker lease establishes
liveness or clears orphan state. Fresh/reset-store availability and orphan
recovery remain explicit activation blockers; no recovery implementation or
new journal, lease service, sweep, or RPC is authorized by this repair.

Display history remains separate from certification. A genuine eligible source
may retain uncertain family paint under an unknown baseline; its original
dispatch order may reject older paint but supplies no known counter proof.
Each family selects one known, paint, or unknown state. Copies cannot promote
paint, and an incompatible newest source leaves an unknown-source barrier
rather than reviving an older compatible count. Pending at capture or
publication denies replacement paint except for the exact verified owner
acknowledgment. An unresolved optimistic-owner stamp stays protected even if
coordination was lost; a zero count in replacement coordination cannot release
it. Only exact settlement or separately approved recovery may do so.

The Inbox writer registers shared ownership before its optimistic cache change.
The task page registers metadata-only ownership before dispatching watch or
unwatch, preserving its DOM-only optimism and one RPC/publication. Registration
failure must prevent an untracked dispatch. Other page action methods retain
their existing dispatch contract.

Already-mounted page and detail-feed owners may subscribe to the exact control
address. A notification is only a hint to reread control state. Unknown or
failed eligibility checks start no network work; eligible changes may schedule
only currently admitted IDs through existing bounded/coalesced owners. Duplicate
hints coalesce, and unmount or context replacement retires queued callbacks.
The notification itself supplies no authority.

Task-specific canonical publication requires full ordinary acceptance and
eligible selected watch-lineage authority before emitting a complete canonical
receipt. A proven newer retained lineage may satisfy that condition; pending
or orphan-uncertain lineage may not. Legitimately omitted optional counters do
not alone prevent completion. Family-only publication cannot retire durable
layers as a complete task read. This conservative boundary can leave canonical
refresh incomplete on fresh/reset coordination until recovery is resolved.

The narrow shared-cache primitive recognizes `DELETE_CACHED_RECORD` only in the
existing multi-record transaction reducer. It does not serialize the sentinel
or change null/no-op semantics. Ordered duplicate-key puts and deletes, atomic
rollback, and post-commit announcements need independent exact-head review
alongside the pending-owner protocol. Passing primitive controls alone does not
close the cross-tab activation blocker.

## Review limits

Client dispatch provenance protects accepted newer observations from
older-issued replies. It cannot order server snapshots within one parsed
millisecond when the older snapshot was requested later. Existing write
responses provide no ordered ordinary-content revision, and projection basis
equality is not server revision ordering. No nanosecond freshness claim follows.

The original RED witnesses, independent controls, and final exact-head gate and
review evidence are recorded on #891. Earlier approvals remain pinned to their
own source heads. This document does not approve the combined checkpoint or
activate the complete task-view conversion. Detail/history integration awaits
#901; general LIST/BOARD remains on compatibility behavior pending #925's usable
exact contract; chooser search remains legacy. UI, performance, and explicit
visible-difference activation reviews remain required.
