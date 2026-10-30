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

## Recovery bounds

Recovery belongs to existing mounted/admitted owners. The task page coalesces
one trailing read behind its active read. The wanted-detail feed retains its
four-read admission limit. Explicit uncertainty has a stable recovery identity;
identical metadata paints and a failed attempt do not create a retry loop.
Unmarked legacy records and summary-only rows do not start unopened-body reads.
There is no project sweep, new background service, or cursor drain.

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
