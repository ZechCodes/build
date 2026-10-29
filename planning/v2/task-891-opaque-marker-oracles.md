# Task #891 opaque marker oracles

## Visible-difference review matrix

| Change | Previous consumer oracle | Canonical/backend oracle and executable fixture | Compatibility boundary |
| --- | --- | --- | --- |
| Opaque unread/watch floors | Legacy prefix regex, truthiness fallback and UTF-16 comparisons | Rust summary/tracking and SQL opaque suffix rules; `taskOpaqueMarkers.test.js`, `taskCountMarkerParity.test.js`, `taskSummaryMarkers.test.js` | Authoritative count consumption requires independent `tasks.counts` support and an accepted response. Without an accepted authoritative total, unsupported bridges use legacy badge calculation. An already authoritative mounted badge retains its last accepted cached total on support retirement/refusal. The numeric differences below are explicit review inputs. |
| Pending-only Assigned/Unassigned | Local `any` checks only real assignee; `none` includes pending-only | Canonical `AssigneeFilter`: any includes pending target, none excludes it, exact actor still real assignee only; `trackerFilterPendingSemantics.test.js`, actual pane `trackerPendingChoiceDom.test.js` | Local correction applies consistently to cached preview and existing backend selection; choosing a new capability does not introduce a second predicate. A bridge with no pending assignment data has no new pending-only rows. |
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
