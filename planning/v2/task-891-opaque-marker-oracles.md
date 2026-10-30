# Task #891 opaque marker oracles

## Visible-difference review matrix

| Change | Previous consumer oracle | Canonical/backend oracle and executable fixture | Compatibility boundary |
| --- | --- | --- | --- |
| Opaque unread/watch floors | Legacy prefix regex, truthiness fallback and UTF-16 comparisons | Rust summary/tracking and SQL opaque suffix rules; `taskOpaqueMarkers.test.js`, `taskCountMarkerParity.test.js`, `taskSummaryMarkers.test.js` | Authoritative count consumption requires independent `tasks.counts` support and an accepted response. Without an accepted authoritative total, unsupported bridges use legacy badge calculation. An already authoritative mounted badge retains its last accepted cached total on support retirement/refusal. The numeric differences below are explicit review inputs. |
| Pending-only Assigned/Unassigned | Original preview checks only actual assignee. The intermediate `taskIsHeld` preview also requires a truthy pending `target.kind`, incorrectly leaving active null/empty/missing-kind targets Unassigned. | Canonical `AssigneeFilter` tests actual-assignee or pending-target Option presence; exact actor still uses actual assignee only. `trackerFilterPendingSemantics.test.js` executes source SQL and real cache DTOs; `tracker_query_choice_roundtrip.rs` verifies actual legacy list, summary list, and get output. | The local correction uses actual assignee or active deferred state, independent of target kind and capability selection. This preserves the canonical any/none predicate; held/readiness remains a separately defined policy. Newly documented null/kind and projected-NUL differences still require checkpoint scope/code/Astra disposition. |
| ASCII label matching | Local `labels.includes` uses exact case | Existing legacy backend `app/tracker/read.rs` and summary selection use ASCII-insensitive matching; `trackerLabelSourceSemantics.test.js`, actual menu `trackerRemoteChoicesDom.test.js` | The correction is independent of summary/choice capability. Legacy scalar RPC label bytes remain unchanged. `API`/`api` and `API\0v2`/`api\0v2` now match both variants; `É`/`é`, surrounding whitespace, NUL and literal U+FFFD remain distinct. This is an explicit local behavior correction, not a claim that the previous preview agreed. |
| Mouse/Enter opaque identity | Parsed HTML attribute turns NUL into U+FFFD on click; Enter keeps original string | Actual `mountFilterMenu` and pane fixtures in `filterMenuOpaqueDom.test.js`, `trackerPendingChoiceDom.test.js`, `trackerRemoteChoicesDom.test.js` | Both actions use original in-memory values, independent of bridge support. Display escaping remains; prefix-colliding IDs and literal U+FFFD stay separate from NUL. |

The checkpoint's independent scope review accepted these documented differences
against their canonical contracts. Separate code and Astra checkpoint review,
final-head gates, and the full #891 integration/activation disposition remain
required. This matrix is not full-conversion or deployment approval.

The pure `spa/src/core/taskOpaqueMarkers.js` model has independent current-SPA
and canonical bridge oracles. The normalized `taskSummaryCache.js` uses it to
retain exact opaque floors, including present empty values, independently of
ordinary row ordering. The legacy unread/attention adapters still use their
existing rules. Activating normalized readers in a user-visible surface needs
an explicit scope/code decision and Astra disposition for the differences below;
this document records those differences, not approval to activate them.

## Canonical source and interface

`bridge/src/tracker/summary.rs` defines `when` as the complete remainder after
the first hyphen, or the complete input when no hyphen exists. `after` compares
`when(id) > when(mark)` when a mark exists. It does not compare an extracted
suffix with a raw marker. `bridge/src/app/tracker/tracking.rs` treats a read as
already covered when `when(read) >= when(event_id)`. Both inputs to the new
comparison helpers must therefore be complete marker IDs. Re-extracting a
suffix that contains a hyphen would change its meaning.

The model exposes:

- `isPresentTaskMarker(value)`: every string is present, including `""`.
- `taskMarkerSuffix(id)`: one explicit first-hyphen extraction.
- `compareTaskMarkers(left, right)`: returns -1, 0, or 1 for optional complete
  markers; null/undefined absence sorts before any string.
- `taskMarkerAfter(mark, id)`: strict source `after` semantics; absence admits
  every entry, including an entry whose suffix is empty.
- `taskMarkerCovers(mark, id)`: a present read marker covers an entry at or
  below its suffix.
- `newerTaskMarker(left, right)`: preserves a complete input, retaining left
  on equal suffixes and returning null for two absent inputs. This selection
  convention does not assert that equal suffixes identify the same record.
- `effectiveTaskWatchFloor(task, timeline)`: one task's stored watch marker, or
  the last maximal watched wire event (`id`) when that stored marker is absent.
  It does not reconcile competing cache records or consume rendered row keys.

Markers are opaque, valid Unicode strings within caller-owned admission bounds.
The model applies no prefix restriction, ULID parsing, trimming, case folding,
NUL filtering, normalization, or length limit. Focused fixtures stay within 128
UTF-8 bytes, including an exact 128-byte marker. Rust `str` compares UTF-8 bytes;
JavaScript compares UTF-16 code units, which reverses some supplementary/BMP
orders. The model compares UTF-8 bytes and rejects JS-only unpaired surrogates
instead of silently replacing them during encoding.

## Visible behavior differences requiring separate disposition

| Input or situation | Current SPA oracle | Canonical source oracle | Potential visible effect if activated |
| --- | --- | --- | --- |
| Unknown prefix or marker without a hyphen | `trackerUnread.taskUnreadKey` rejects it; attention may ignore the read marker | Preserve and compare its source suffix | Unread lines, counts, and attention reasons may change |
| Empty marker or empty suffix | Unread key extraction rejects it; attention's truthiness admits all after an ignored marker | Empty string is present; equal empty suffixes are read | A previously shown unread entry or attention reason may clear |
| Multiple hyphens | Unread key extraction rejects it; attention extracts at most one segment and can discard the rest | Compare the complete first-hyphen remainder | New entries may become unread or older entries may be covered |
| Supplementary scalar versus high BMP scalar | Both current adapters use JS UTF-16 ordering | Rust UTF-8 lexical ordering | Read coverage and newest-marker selection may reverse |
| Present empty or unrecognized stored watch floor | Current watch helper falls back to a recognized watched event | `Some("")` and every other present string bypass fallback | The watch boundary and unread count may change |
| Equal-suffix watched events | Current helper retains the first recognized event | Rust `max_by_key` retains the last maximal event | Returned floor ID changes; equal suffix coverage stays equal |

NUL, whitespace, decomposed Unicode, and embedded hyphens remain significant
bytes. No migration may silently collapse these values into absence. Legacy
`ic-`/`ie-` renaming already produces the same extracted suffix for ordinary
markers; the canonical model needs no rename operation.

The focused test uses literal current-SPA expectations, a separate Buffer UTF-8
source oracle, and source-text guards for Rust `when`, `after`, watch fallback,
and inclusive read-through. This requires no bridge launch, full test suite,
new RPC, durable synchronization, or network change.

## Pending assignment presence and serialized-state proof (#900)

The pending-assignment correction changes only local Assigned/Unassigned
filtering in `trackerFilters.js`. The canonical predicate is
`task.assignee.is_some() || pending_target.is_some()` in
`bridge/src/app/tracker/edits.rs`; Unassigned requires both to be None. Exact
actor selection compares only the typed actual assignee. Store list extraction
in `bridge/src/store/tracker.rs` filters the current intent to `waiting`,
`queued`, `provisioning`, or `failed`, then maps it to `&intent.target`.
Consequently an active intent with `Value::Null`, `{}`, or an empty kind still
provides Some; target kind truthiness does not determine pending presence.

The local predicate now uses actual assignee presence or
`taskHasUndeliveredAssignment(task)`. Its correctness depends on retaining the
active-state fact through serialization and cache projection. That dependency
was checked separately from the hand-built consumer fixtures:

- Actual RPC producer test
  `pending_presence_wire_survives_null_and_missing_kind_across_list_get_summary`
  in `bridge/src/app/tests/tracker_query_choice_roundtrip.rs` imports 42 complete
  isolated canonical metadata cases: four active states plus absent, consumed,
  and cancelled controls; each with null, missing-kind `{}`, or empty-kind
  target; each with and without a real typed assignee. After projection repair,
  it checks actual legacy `tasks.list`, summary `tasks.list`, and `tasks.get`
  output, as well as canonical any/none/exact summary selection. Active output
  retains a deferred-assignment object and active state even for null target.
  Full list/get preserve the target itself. `provisioning` serializes as
  `queued`, retaining active membership; absent/consumed/cancelled controls
  have no active deferred object.
- `trackerFilterPendingSemantics.test.js` extracts the exact trusted
  `enrich_patch` SQL from `bridge/src/store/task_summaries/sql.rs` and executes
  it in an isolated in-memory SQLite database using pinned Node's
  `node:sqlite`. Seventy saved-intent/assignee cases include null, `{}`, empty,
  leading-NUL, and Unicode target kinds across the same state controls. The
  actual serialized summary rows then pass through `summaryPatch`,
  `tasksRecord`, and `taskRecord` before the local filters. These DTOs preserve
  the active-state fact. Source guards also pin the shared full-list/get
  `extend_task_summary` → `extend_assignment` serializer chain.

| Pending source | Intermediate `taskIsHeld` preview | Canonical any/none and corrected preview |
| --- | --- | --- |
| Active intent, null target or missing/null/empty/false/zero kind, no actual assignee | Unassigned | Assigned; exact actor remains unmatched |
| Active intent, truthy Unicode/whitespace/array/object/nonzero/true kind in a raw row | Assigned | Assigned; exact actor remains unmatched |
| Active intent, leading-NUL kind after summary display projection | Can become Unassigned because SQLite display `substr` yields an empty kind | Assigned because serialized active state survives |
| Absent, consumed, or cancelled intent, no actual assignee | Unassigned | Unassigned |
| Actual typed assignee, with any tested pending state or target | Assigned | Assigned; only the actual actor matches exact selection |

The leading-NUL display limitation is demonstrated by the executable source-SQL
fixture, **not** by the 42-case RPC test. Summary target projection can lose or
truncate kind display bytes while preserving pending state. This is also why
`pending_present` and `pending_held` in `query_sources.rs` remain distinct:
the former records active-intent presence; the latter preserves the separate
held policy's raw target-kind truthiness. The existing `taskIsHeld` readiness
and grouping helper was not changed, and this correction does not redefine
held/readiness queries or approve a backend policy change.

Verification at test commit
`dff68e6d622dbf29eb7e8e97176a831663009542`: the fully qualified named Cargo
filter executed **one test**, passing all 42 RPC cases; the two focused JS files
(`trackerFilterPendingSemantics.test.js`, `trackerFilters.test.js`) passed
**121 tests**. Source-SQL/DTO proof first landed in `3af8ecac0`. The initial
short Cargo filter combined with `--exact` executed zero tests and is explicitly
excluded from evidence. These are verification tests for existing producers,
with no manufactured red-test claim; the local predicate's earlier TDD test
commit `965dad938` independently recorded 31 expected failures before the fix.

This evidence applies to the tested current canonical producer/read paths with
complete synchronized saved intent metadata. It does not assert compatibility
with arbitrary older deployments or unsupported imports. No missing serialized
state fact was found on the tested paths. Scope/code and Astra review of the
visible correction, final-head gates, and integration/activation disposition
remain required; these tests do not authorize deployment or alter #925's
separate held/readiness contract.

## Actual project badge aggregate evidence

`spa/test/taskCountMarkerParity.test.js` now exercises the actual
`followProjectTasksUnread()` follower over real IndexedDB list and detail
records. Each case first asserts the current `taskUnreadCount()` result and the
badge's legacy number. It then enables persisted counts support, fetches one
exact project aggregate through `createTaskAggregateReader()` with a scripted
`tasks.counts` RPC, and asserts the same follower's authoritative number. The
cached detail and its current-SPA unread result remain unchanged. This isolates
the badge's change of authority from any change to the legacy unread adapter.

The canonical expectation comes from a separate Buffer UTF-8 source oracle,
with literal count witnesses; it does not import `taskOpaqueMarkers.js` or reuse
the badge implementation. These fixtures contain agent comments and user watch
events on one watched, unfinished task, so the independent oracle checks both
read and watch floors for every eligible comment. The test pins these exact
source contracts:

- `bridge/src/tracker/summary.rs`: `unread_since_mark`, `watch_started_after`,
  `after`, and `when` apply both strict floors, fall back only for an absent
  watch marker, retain the complete first-hyphen remainder, and use Rust string
  ordering.
- `bridge/src/store/task_summaries/sql.rs`: `opaque_suffix` uses BLOB slicing
  through the first hyphen, preserving embedded NUL and subsequent hyphens.
  `sql/activity.rs` derives unread counts using nullable read floors and
  `COALESCE` of the stored watch floor with the maximal watched fact.
- `bridge/src/app/tests/tracker_query_exact_text.rs`:
  `watch_floor_missing_and_explicit_empty_remain_distinct_in_actual_counts`
  asserts actual `tasks.counts` totals of 1, 2, and 1 for `None`, `Some("")`, and
  `None`, respectively. This is the backend witness for the empty watch floor;
  the new SPA test does not claim to launch that RPC boundary test itself.
- `fixtures/api/v1/tasks.counts.json`: `canonical_marker_domain` explicitly
  preserves the distinction between absence and present empty watch floors,
  and `refusals` supplies the unsupported-marker refusal used by the test.

The measured visible numbers are separate oracles, with explicit differences:

| Fixture | Current badge before aggregate | Canonical aggregate badge | Difference |
| --- | ---: | ---: | ---: |
| Missing or null watch floor, with a watched event between two comments | 1 | 1 | 0 |
| Present empty watch floor on the same history | 1 | 2 | +1 |
| Present unknown-prefix watch floor `unknown-A` | 1 | 0 | -1 |
| Present no-hyphen watch floor `A` | 1 | 0 | -1 |
| Present multiple-hyphen watch floor `te-0-Z` | 1 | 2 | +1 |
| Present raw NUL watch floor | 1 | 2 | +1 |
| Present raw space watch floor | 1 | 2 | +1 |
| Unknown-prefix or no-hyphen read floor `unknown-B` / `B`, comments `tc-A`, `tc-C` | 2 | 1 | -1 |
| Unknown-prefix or no-hyphen comment `unknown-C` / `C`, read floor `te-B` | 0 | 1 | +1 |
| Multiple-hyphen comments `tc-A-X`, `tc-A-Z`, read floor `te-A-Y` | 0 | 1 | +1 |
| NUL in recognized read/comment suffixes | 1 | 1 | 0 |
| Whitespace in recognized read/comment suffixes | 1 | 1 | 0 |
| High BMP comment `tc-\uE000`, supplementary read floor `te-\u{10000}` | 1 | 0 | -1 |
| Supplementary comment `tc-\u{10000}`, high BMP read floor `te-\uE000` | 0 | 1 | +1 |
| Composed `é` comment after decomposed `e\u0301` read floor | 1 | 1 | 0 |
| Ordinary 26-character ULID read-through control | 1 | 1 | 0 |
| Ordinary ULID read plus watch-floor control | 1 | 1 | 0 |

NUL and whitespace therefore have two different witnesses: recognized
`tc-`/`te-` suffixes retain their bytes under both consumers in the selected
controls, while unrecognized raw stored watch floors are present canonically
and trigger fallback in the current SPA. The Unicode witnesses separately
show both directions of the UTF-16 versus UTF-8 ordering reversal. Matching
control counts are limited parity evidence for those inputs.

A further actual-follower case fetches the canonical total of 2 for the present
empty watch floor, then refuses the next explicit read with the fixture's
nonretryable `task_query_marker_unsupported` response. The fetched total, basis,
and legacy timeline remain cached, the badge remains 2, and the reader makes no
automatic retry or list traversal.

This adds verification of the existing counts path, not a new implementation;
there is no manufactured red-test claim. The evidence exposes visible
semantic changes and does not approve them. An explicit scope/code decision and
Astra disposition remain required before final activation. Navigation's
project/agent ownership split is outside this project-badge evidence.

## Action-cache raw marker and counter retention

Confirmed task actions now reconcile their full-task response across detail,
legacy catalogue/query/page rows, and normalized summaries in one transaction.
This action-cache path uses the canonical optional-marker and UTF-8 suffix
ordering above even when the device has no summary capability. Ordinary legacy
reads, unread adapters, and rendering helpers retain their existing behavior.
This is an explicit action-cache behavior change, not a claim that the previous
legacy floor helper had canonical parity or a capability-dependent migration.

`taskActionReplicaFloors.test.js` measures these retention differences directly:

| Held / incoming read floor | Action-cache result and paired counters | Previous legacy detail floor helper |
| --- | --- | --- |
| Present empty `""` / absent | Retains present empty and held counters | Ignores empty as an unrecognized legacy mark |
| `tc-20\0` / `te-10\0` | Retains the complete NUL-containing held floor and its counters | Retains that recognized held floor, but did not reconcile its counter family |
| `tc-\u{10000}` / `te-\uE000` | Retains supplementary held floor and held counters | UTF-16 comparison selected the high-BMP incoming floor |
| `tc-\uE000` / `te-\u{10000}` | Accepts supplementary incoming floor and incoming counters | UTF-16 comparison retained the high-BMP held floor |

The complete raw marker is retained; suffix equality does not imply marker
identity. Total unread counters follow the maximum read/watch floor. Mention
and agent-comment counters follow the read floor independently. Retaining a
stronger floor with unknown held counters removes weaker incoming counters
rather than attaching them to that floor. An authoritative unwatch clears the
watch floor and all watch counter/session lineage. No count arithmetic is used.
An older content snapshot can advance marker authority while retaining newer
content and held counters; normalized uncertainty stays latched until a
complete covering authoritative summary supplies the counter families.

Every supported action owner takes an explicit shared dispatch token after
admission awaits and immediately before its mutating RPC. The transaction
checks that token against the per-task action note, so a late older assignment
or dependency reply cannot reverse a newer unwatch. A compatibility call lacking
a token may update only before a proved action note exists, never receives
forced watch authority across older content, and never promotes arrival order
into dispatch authority. Acceptance refusal and quota failure publish none of
the detail, list, page, summary, invalidation, or read-note updates.

### Current watch-session authority

Watch-session identity is distinct from monotonic read and dismissal coverage.
A current full watch header may replace a lexically larger prior watch floor,
including when the client missed the intervening unwatch. This preserves raw
absence, present empty, and lower-valued current markers. The normal manual
watch path creates a Watched event and fills its absent floor with that event's
ID (`tracking.rs` and `TaskWrite::event`); imported opaque prior floors can still
sort above that new event ID.

The local `watch_read_order` records the request that established the current
watch header. A proved full action uses its issued token after checking the
per-task action note and captured session inside the joint transaction. A full
summary uses its local request order independently of the ordinary body
`updated_at`, so a future-dated imported body cannot prevent watch recovery.
Body, timeline, and PR preservation retain their existing authority rules.
Earlier issued action/page replies cannot restore a prior session. Records
without this metadata conservatively use their existing local read order;
compact or unissued compatibility patches receive no new reset authority.

Changing the session retires the prior watch counter/activity lineage across
all action replicas. Read and dismissal markers still advance monotonically;
if retaining a stronger read floor overtakes the new counters, those counters
remain unknown rather than inheriting values from the old session. A complete
covering current summary can recover an independent fresh-basis query without
ordering or parsing its opaque basis.

`taskWatchLineage.test.js` supplies real-IDB single-case evidence for the
`opaque-zzzz` to current Watched-ID transition in detail, catalogue, query, page,
and normalized records, independent summary recovery, future-body recovery,
null/empty/omitted floors, read/dismiss retention, stale action/page protection,
and in-transaction session replacement. The action and independent-summary
witnesses were observed failing before implementation in RED commit
`5eef64373`; their retained artifacts are
`/tmp/task-900-watch-lineage-red.log` and
`/tmp/task-900-watch-summary-red.log`. This records the cache authority repair
and its focused evidence; it makes no activation or completion claim.

The two prior cross-floor action counter witnesses deliberately change their
expectations under this session rule. A proved watch change from `te-05` to
`te-30` with read `tc-10` below held read `tc-20` accepts total 0 at watch floor
30, but leaves attention counters unknown instead of transplanting old-session
zeros. The reverse watch change from `te-30` to `te-05` with read advancing from
10 to 20 accepts the new-session total 4 instead of retaining old-session total
0. Their original failing single-case outcomes are retained in
`/tmp/task-900-watch-prior-family0.log` and
`/tmp/task-900-watch-prior-family1.log`. A separate unchanged-watch-floor control
still accepts the total at watch floor 30 while retaining attention counters
paired with the stronger read floor 20, preserving independent family coverage.

### Watch recovery does not certify retained ordinary content

A current watch observation can recover its counters while a newer cached
ordinary payload remains protected. Such a row retains `basis_uncertain:true`
and no certified row basis. A newer local watch request never labels retained
B1 title/status/content as one complete B2 snapshot. This also applies when the
watch header is unchanged: a Ready-filter page cannot certify its retained
`status:done` row merely by advancing the watch observation clock.

`taskSummaryMixedBasis.test.js` observes the c78 failure in RED commit
`980c45624`, with separate single-case artifacts
`/tmp/task-900-mixed-basis-red.log` and `/tmp/task-900-mixed-watch-red.log`.
The tests reopen persisted IndexedDB records and accept another still-partial
snapshot while retaining uncertainty. A later full summary that actually wins
ordinary-content authority and covers the marks can restore a single basis.
The future-body watch tests now explicitly distinguish recovered counter
families (`needs_unread_refresh:false`) from an uncertified combined snapshot
(`stale:true`). Their prior incorrect `stale:false` expectations were retained
as observed single-case failures in
`/tmp/task-900-mixed-prior-future-body.log` and
`/tmp/task-900-mixed-prior-complete-counters.log` before correction.

`readSummaryQuery.complete` continues to mean that the fetched page chain is
exhausted. Snapshot certification additionally requires `!stale`, a matching
row basis, and no mixed-row uncertainty. Exhaustion alone never certifies
absence: the actual reference resolver keeps an unlisted task unknown, and
only an explicit bounded point result with `status:missing` can mark it missing.
