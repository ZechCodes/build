# #925 retained-generation and consumer-domain addendum

Design-only response to #925/c/tc-01M64YV8SX8JKSJ55QFDBJK2QV and #891's
consumer review. The research packet at `821a605438822277cb4c62c2d7bf1e164dda049f`
remains unchanged. Its measured restart-on-change approach is **not approved
for production, support-envelope/cost acceptance or #891 activation**. No
optional timing run, production edit, new base, dispatch or dependency mutation
is included here. New proposals below have not been prototyped or measured.

## Foundation status, scope and ownership

#892 is `task-01M5WEFKMD8R9B8HNDQ7F02KF6`, currently undispatched and without an
implementation owner, frozen API, schema or foundation implementation. The epic
lead owns coordination. Its approved intent is the #887 plan and recovery
addendum (#887/c/tc-01M5WC725H9WG86KQNDZH008JX and
#887/c/tc-01M5WDTY9K8288DND6RQX08DTN), approved at
#887/c/tc-01M5WE8NPN9Y89CA8PKQRCED6A: consistent bounded snapshot leases,
versioned projection/snapshot pages and a transactional journal watermark.
This addendum requests the smallest task-projection use of that foundation;
it does not describe an existing service or settled API.

Proposed ownership is one shared #892 foundation for transactional revision
publication, retained bounded task/card facts, snapshot identity/lease,
retention admission, expiry, crash recovery and version GC. #925 remains the
consumer that seals caller query inputs and builds private selected membership,
counts, group/status indexes and cursors. It must neither run another snapshot
or journal service nor reconstruct historic state by replaying events itself.
#892's planned retention worker/admission, once implemented, owns shared retirement scheduling;
selection-specific output rows register bounded cleanup with that same owner.
There must be one authority for source lease release and physical-byte charging.

Minimal proposed dependency order, for epic disposition rather than board edits:

1. Define and assign the retained **task-projection subset of #892**: exact
   fact/card coverage at one revision, bounded as-of enumeration/lookups,
   finite leases and retention/GC, plus the underlying publication boundary.
2. Validate that subset independently with writes during capture/build/page
   reads and bounded physical-version/retention accounting. It need not wait
   for every bootstrap/non-SQL scope or the entire #893 consumer rollout.
3. Only then implement/validate #925's exact selection consumer, if separately
   authorized; it depends on that subset. Source-domain producer choice remains
   another disposition, coordinated with #901's shared Store/projection work.
4. #891 can continue independent cache/paging/compatibility work now. Its exact
   List/Board activation waits on approved retained foundation + exact selection
   integration, not completion of all #892 journal consumers. #893's existing
   broader #892 dependency remains separate.

Do not dispatch #892, modify its task dependencies, or claim activation is
unblocked merely by approving this design addendum. If tracking a foundation
milestone requires another child task, the epic lead assigns it within #892's
ownership. No second service/parallel journal is proposed.

## Required retained-generation boundary

A build needs a finite source generation S that stays readable throughout
private evaluation, atomic publication and subsequent membership/card reads.
A project epoch with restarts does not provide that, and a snapshot marker
without retained versions does not provide it either.
The retained identity binds #892's incarnation/revision/root and lease schema;
it is a new foundation concept. Today's opaque #900 task-query basis is not a
journal cursor and must not be reinterpreted as S. The selection source lease
pins versions/root access, not an unlimited journal replay interval. Any later
live catch-up uses #892's separate scope cursor/floor rules; an overtaken replay
floor requires explicit resync, never a relabeling of this as-of-S answer as
current or a second #925 event reader.

Before admitting any source lease, #892 must have complete, transactional
version coverage for the selected project's task header/state/status/number,
actual/pending assignment presence, exact labels or explicit validity flags,
attention/read/watch/dismiss facts, blocked/readiness/priority, and every bounded
non-decoration card field. A task mutation and its relevant fact/card versions
must share the canonical SQL transaction at the required #892 publication/revision
boundary. This is an approved consistency requirement, not an implemented
#892 transaction API. Move/reparent,
remove/recreate, history repair, dependency and deferred-intent producers cover
both old/new owners. Uncovered migration/repair remains not-ready. Canonical
legacy writes/data remain accepted; selection support cannot silently alter them.

Capture the source revision/root, incarnation, covered schema/domain and lease
reservation atomically in a short transaction. This operation records fixed
metadata and retained references; it must not copy/COUNT/GROUP BY the project.
Pin read/watch/attention values as of S; do not splice newer unread metadata
into an old selection. Sealed caller K/W/coordinator/config inputs are a separate
explicit input generation and need not represent the same real-world instant
as source S: exactness is defined by that declared pair of identities.

A later quantum opens a short read at S, reads bounded exact facts/card versions,
then closes it before checkpointing private work. Writers can advance past S
without restarting this build. EOF plus one publication transaction verifies
the lease is still valid and all validation/progress checkpoints are complete,
then seals the exact products. It does **not** require live head == S. Output
pages use retained card versions at S for selected task IDs, not `tasks.list`
against today's basis. Thus an unrelated or selected-project write after seal
cannot strand the first page or substitute newer row fields. Live identity
names/availability remain separately fenced decoration, never grouping authority.

No SQLite read transaction remains open between quanta or client requests; no
WAL reader is pinned for a lease lifetime. A background copy of today's rows
across transactions cannot retrospectively supply S. Version preservation must
exist before S, or a reviewed copy-on-write boundary must preserve every value
before its first post-S mutation. Full-project copy transactions, lazy live-row
hydration and final unbounded validation are rejected.

## Bounded version access requirements, not a new schema commitment

The #892 owner must choose the mechanism; two compatible directions are an
immutable/versioned projection root or indexed per-fact versions. Both must
supply fixed-size handles and short bounded reads. A generic `valid_from<=S`
filter plus LIMIT is insufficient: it can walk unlimited historic versions.
Per-task labels may be arbitrarily numerous; separate relation/fact continuation
and byte/probe/VM accounting remain required at S.

A relational implementation could use a stable, unique project/task-key
registry with a creation sequence, and bounded keyset enumeration restricted
to keys created no later than S. Each key gets a point seek to its latest version
at/before S (including tombstone); retained exact source bytes decide selection.
Relation keys get their own bounded registry/cursor and as-of seek. This is an
illustrative access shape, not an assigned schema. The owner must prove:

* Physical registry keys/version candidates are charged, including deleted keys
  and stale relation entries; no hidden antijoin, history walk or sort. Live
  cardinality N alone does not bound an unpruned registry. The finite key range
  at S excludes newly created keys after S, so sustained creation cannot move
  EOF forever. Version probes must seek a full indexed key rather than walk
  every rewrite of a hot task.
* A key/version needed at any admitted S survives GC until release. Project
  moves, leave/reentry and delete/recreate cannot duplicate task membership or
  reuse a cursor identity; stable logical identities and incarnation fences are
  explicit. Any missing needed version is a failed/invalidated generation,
  never an absent task or exact zero.
* Cardinality/byte reservations use maintained bounded counters, never a
  request-time aggregate. The physical-version/registry amplification factor
  and permitted retained roots must be measured before admitting a domain.
  Historical tombstones/GC debt consume the same quota as visible records.
* Repeated mutation cannot produce uncharged version growth. Retain only
  versions/immutable roots actually needed by admitted snapshot references and
  journal policy, or cap and retire leases explicitly under retention pressure.
  Do not copy all task relations on one quantum or attach per-query predicate
  reevaluation to writes. Any per-retained-root producer work is separately
  bounded/admitted and measured; snapshot retention is not free producer work.

A retained card is a bounded projection payload, not an unlimited canonical
body/detail. #901 detail/timeline scope is unchanged. Exact membership fields
must not come from clipped display values. As-of unsupported-source flags must
be retained with the same versions; fetching a newer canonical value cannot
repair an older generation silently.

## Coherent caller staging and cache publication

Before staging, #891 captures one immutable input manifest in its cache: exact
known-agent set, workspace-residence set, coordinator resolution/interpretation,
ordered configured columns/fallback version, attention rule and filter version.
An atomic metadata transaction captures its root/revision vector. Every input
chunk reads that retained manifest version; it does not enumerate whatever the
latest mutable roster/config happens to contain. The manifest describes complete
source sets, not a partially received directory page.

Server staging binds every handle/chunk, replay receipt, final seal and build
admission to the same manifest identity/vector and chunk sequence. Config
ordinals and duplicates are preserved. A changed vector midway cannot seal
as the old generation. Counters/digests prove the received set's integrity,
not that a client read a coherent cache: the client owns retained-manifest
capture and version checking; authentication does not prove those source facts.
If the client has no retained input representation, any relevant revision change
fences/cancels the staging attempt; chunks from different revisions cannot be
combined. Returning to equal values does not reset a monotonic local epoch.
A manifest can either finish from its retained version (and remain explicitly
historical) or be superseded, never silently relabeled as current.

After publication, one cache transaction installs exact metadata, membership,
cursors and fetched S-card copies/version references under the pair (input
manifest, source lease S), after local request/mutation-epoch checks. A later
normalized live row cannot overwrite retained S cards. A new local mutation
keeps immediate optimistic behavior but marks the canonical generation stale;
its exact counters remain historical S facts. An expired lease prevents fetching
additional S rows; already loaded coherent cached rows/metadata stay tagged S.
No filter or new roster gets another generation's exact total. Rendering reads
these support states from cache, with no connection-state membership branch.

## Retention, expiry, cancellation and cleanup cost

The lease has a nonrenewable maximum build+page-read age and explicit owner,
project, snapshot schema and retained-byte reservation. A finite initial page
window can follow successful seal only within the already reserved maximum
age/bytes; sealing must not restart the clock indefinitely. Slow/abandoned
clients expire explicitly. Renewal/new selection is a new admission decision,
not an automatic cursor-drain or unbounded retry policy.

#892 owns minimum retained revision/root references, indexed due-expiry work,
reference accounting, pressure refusal, process-restart recovery and physical
reclamation. #925 cancellation/expiry first fences private/output access by an
O(1) metadata change. The foundation releases source references only when no
build/page operation can use them and its in-flight read fence has drained.
A bounded read takes a short-lived reference and validates the same lease/
incarnation at publication; racing expiry/GC cannot expose missing half-pages.
Process epoch fences old replies; a durable lease may resume only if its roots
still exist and its original deadline/incarnation remain valid.

Cleanup removes <=64 owned records per proposed turn (measurement-backed
research ceiling, not production approval), without cascades/whole-selection
DELETE/DROP. Charge private membership/status/input/receipt rows, retained
header/relation/card versions, registry/tombstone debt, indexes and WAL/free-page
space. Source release may only decrement fixed-size root/reference metadata;
physical version retirement proceeds through #892's bounded maintenance. Finding
all consumers/versions by a terminal full scan is not allowed. Indexed reference/
expiry/floor checks and physical candidate budgets must be part of foundation
acceptance. Existing bounded physical-page maintenance stays its single owner.

When retention capacity cannot be reserved, refuse new leases before work.
If existing capacity is threatened, refuse further admissions and apply the
reviewed priority/expiry policy; explicit affected-generation retirement may be
necessary. Never silently discard an active S version or fail canonical writes
because a selection's retention budget was underestimated. This is a finite
support boundary, not unconditional progress for arbitrary write rates or
arbitrary user think time. Worst-case pressure/cleanup behavior and original
write acceptance need tests and cost disposition before implementation.

## Quantified candidate envelope and missing measurements

The original data establish costs of the restart prototype only. Retention
would remove mutation-triggered restarts **conditional on complete retained
coverage and capacity**, while adding producer/version lookup/storage/GC costs.
It cannot fix insufficient throughput by itself. No actual supported envelope
for the new mechanism has been measured or approved.

A useful narrow acceptance cohort is one 10k-task selection with 1,001 roster
and config entries, at most 1,001 selected statuses, default first 25 List cards,
finite page use and no unsupported source facts. Account for every phase:

| Component | Original research turns or requirement |
| --- | ---: |
| Input staging + admission + build/seal | 187 measured turns |
| Full 1,001-record column metadata traversal at proposed 100/page | >=11 additional calls, not measured retained implementation |
| First 25 cards at 64KiB maximum row and 1MiB encoded envelope | >=2 calls in the largest-row case; envelope overhead must be charged |
| Private/input cleanup | 209 measured turns |
| Retained source/version admission, lookups, preservation and GC | additional unknown work G and scan amplification A |

The simplified one-job all-phase lower-bound arithmetic is 409 turns plus new
retention work and any lookup amplification: >=40.9 service seconds at ten
global turns/second, before scheduling/IO/user delay. Those 11 metadata calls
are a capacity calculation, **not permission for automatic client drain**;
each foreground action has finite request budget and explicit continuation.
Three jobs sharing staged inputs need at least 505+33+6+555=1,099 turns before
new retained-generation work, leaving only 101 of the old 1,200-turn envelope.
Twofold build-turn amplification alone raises that to 1,573, which fails.
Thus neither the old three-job passing scan nor sixteen-job quota can be
adopted as a supported retained-service envelope. Quiet 100k already exceeds
the deadline without retained-version overhead.

Proposed starting measurement envelope: **one active build globally**, one
retained source generation per project, <=10k live/retained candidate keys,
<=1,001 input entries per demonstrated roster/config set, and separately bounded
physical relation/version keys and retained payload bytes. These are cohort
limits for validation, not silent truncation or production supported-domain
restrictions. More valid data must return explicit capacity/not-ready refusal
until a larger envelope is proved. If the source has many tombstone/relation
keys, 10k live tasks does not qualify by itself. 100k remains explicitly outside
this candidate's demonstrated deadline, not an exact-zero fallback.

The eventual scheduler needs reserved service for input/admission, snapshot
retention, active build/page work **and cleanup**. For each admitted operation,
reserve estimated total turns/bytes with a separately measured safety bound and
absolute expiry; charge shared roots once and every owner's output separately.
Use round-robin bounded turns within fixed classes, a measured minimum cleanup
share, foreground priority and bounded queued admissions. A ceiling of ten
turns/s gives no minimum throughput guarantee: actual service rate, lock
contention and queue time must be measured. Old cleanup debt must consume new
admission capacity, not accumulate behind fresh work forever. A cleanup share
reduces build capacity and belongs in the deadline calculation.

Additional retained storage is proportional to required header/relation/card
versions plus indexes/registry/GC debt; it is not only selected M. At 10k, even
one <=64KiB retained card per task could approach 625MiB of payload alone,
compared with the original 3.9MB selected-index delta. Do not reuse the old
64MiB/build or 256MiB/global proposals as if they cover full retained payloads.
Sharing roots avoids duplicate source versions across selections but does not
remove bytes. Measure ordinary and maximum-row distributions, hot-rewrite and
high-churn/tombstone cohorts, migration/repair, baseline writes and version GC.
A quota that cannot retain the cohort must refuse or shrink the **admission
cohort**, never clip cards or fields. No source retention/cost choice is accepted.

## Source availability and consumer witnesses

Project-wide indexed refusal remains a research baseline, not approved general
availability. A malformed task outside F must not automatically make ordinary
supported selections unusable without explicit disposition. Compare:

* Conservative project invalid-index refusal: low request work, potentially
  refuses unrelated off-filter rows (the frozen model's 10/10 open queries).
* Private selective validation at S: evaluate only lossless cheap dimensions
  first. If they prove an invalid record outside F, it contributes nothing;
  otherwise refuse. No result/discovery/traversal is published before complete
  validation at S. Unknown dropped fields cannot prove exclusion. Retained
  versions make completion stable, but source validity is still a prerequisite.
* Lossless exact fact versions: recover supported larger label sets and retain
  their real predicates. Costs include O(relation changes), snapshots and
  activation/backfill; the research 10k-label replacement already uses 270,123
  VM steps. Refusal/retention must preserve canonical write acceptance.

The new three consumer-domain tests are witnesses, separate from the frozen
22 tests and 32-scenario measurement. The prototype worker remains unchanged:

* Label probes compare exact strings. Raw `API`/`api` produces zero in that
  prototype, so it does not implement canonical ASCII-insensitive matching.
  Measured cohorts used already normalized ASCII labels. A **declared input
  adapter** must fold ASCII A-Z on both canonical source facts and selected
  label sets before exact probes; changing only the filter is insufficient.
  The test-only adapter proves API/api and API\0v2/api\0v2 match, É/é remain
  distinct, and NUL versus literal U+FFFD remain distinct. Whitespace is not
  case normalization; source validation/normalization must preserve that
  distinction rather than trim it into a successful match. Raw invalid labels
  may still cause explicit domain refusal under the chosen source option.
* The initial production numeric wire proposal is **0..9,007,199,254,740,991**
  (Number.MAX_SAFE_INTEGER) with exact integer validation before seal/dispatch,
  including off-page potentially selected tasks and reference lookup inputs.
  Above that domain, explicitly refuse; never round, saturate or alias task IDs
  through a Number/parse path. The unchanged Python prototype permits i64 and
  its test demonstrates acceptance above MAX_SAFE_INTEGER plus a binary64
  collision. That is a documented gap, not consumer support. A future decimal-
  string wire needs independent versioned capability and exact comparison,
  cursor/reference-lookup/cache/key handling on both sides before enlargement;
  merely putting a decimal string on the wire does not fix an existing Number
  parser. No such path is implemented or approved here.
* `any`/`none` requires active-intent target presence in the canonical producer
  semantics, distinct from held-kind truthiness. #891 is auditing serialized
  DTOs: absent intent versus present target null/{} or empty kind can differ
  from in-memory Option semantics. Until wire provenance/presence is verified,
  no local null-vs-absent parity claim follows. Keep a dedicated presence fact
  at S with producer/DTO fixtures; never infer it from the displayed target or
  collapse it into readiness. #891 owns its separate local fix/review.

Required new foundation/consumer fixtures: mutation between every capture/scan/
seal/page step without losing S; late first page after continuous writes;
lease expiry racing page response and bounded GC; crash during version publish/
release; project move/reentry/delete/recreate; hot-version seeks and physical
candidate bounds; pressure policy preserving canonical writes; input manifest
changing mid-staging, including config ordinals and coordinator; and these
ASCII/NUL/whitespace/JS-number/serialized-presence boundaries. This addendum's
numbers are conditional capacity arithmetic from old measurements, not new
retained-service performance evidence. Fresh architecture disposition and epic
ownership/cost/activation decisions still precede implementation.
