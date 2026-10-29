# Canonical sources for bounded task-query predicates

This audit follows the request-changes review of
`0d3e81643f66846db76258809ff4888a1c8a3d76` against base
`12b8f869bd94e259666df69f1a8c0cde5c695982`. Findings below refer to that exact
reviewed head; the production repair is now committed as
`eca9e4692` (sources) and
`f2769d34ddf9cfb9614a6b2d475b6b2f992d7f32` (query/RPC integration). This is an implementation
source audit, not independent final scope/code/Astra approval. The integration symbols below have been inspected in the working repair source.
Focused regression outcomes below belong to the frozen integrated working-tree
repair and emitted test binary, before production commits; they are not
clean-head whole-suite runs. Full/lint/release gates and independent final
reviews remain separate requirements.

## Preserved review findings

Astra requested changes for two independent P2 findings at `0d3e81643`:

1. **Unsupported canonical timestamps became supported.**
   `bridge/src/store/task_summaries/query_projection.rs:156–159` parsed display
   summary values. `sql.rs:152–154` and `sql/activity.rs:9–10` applied SQLite text
   `substr`, which stops at embedded NUL. A canonical timestamp such as
   `2026-10-01T00:00:00Z\u0000junk` could become a supported timestamp; a leading
   NUL could become Unknown. Classification must precede display normalization
   and survive every retained-state refresh and repair.
2. **Lossy assignee IDs corrupted exact aggregates and exclusion.**
   `query_projection.rs:189–191` used an ID shortened by `sql.rs:117`.
   `AgentA\u0000suffix` could contribute to `AgentA`; `\u0000AgentA` could
   contribute to the empty project bucket and count the task twice. Both inputs
   fit the existing nonempty/128-byte ID boundary. Exact positive/exclusion
   predicates and bucket keys must use the canonical identity.

These findings are not replaced by the earlier code/scope approvals. They stay
request-changes findings until repairs, regressions and fresh independent
exact-head reviews establish otherwise. The original timestamp-domain and
search exclusions remain in force.

## Subsequent independent review: two new P2 findings

At exact head `484537214`, independent Astra review closed the original `0d3`
timestamp and assignee findings after the canonical-source repair, and requested
changes for **two new, separate P2 findings**. The source repair is now frozen in `fa8a4ff9b`; the two findings remain
review-open here. Prior full-suite/release successes do not establish the
missing guarantees.
The original findings, failed attempts and repair evidence above/below remain
historical evidence rather than being erased or relabeled.

### Actual meaningful-history work

At `484537214`, `bridge/src/store/task_summaries/query_projection.rs:205–206`
combined `?9 IS NULL OR f.ulid>?9` inside the meaningful-history EXISTS. An index
name alone did not prove a bounded range: the actual production INSERT could
seek only `task_id` and scan the task's meaningful history when a high dismissal
floor excluded all entries. This affects ordinary maintained legacy-attention
facts, rather than just a standalone equivalent SELECT.

The added `query_projection::cost_tests` executes the exact shared
`QUERY_FACT_REFRESH` production statement, forces the eligible watched/unread
comment branch, records SQLite `StatementStatus::VmStep`, and checks membership
as well as physical work. `/tmp/task900-refresh-cost-red.log` records **1 passed,
1 failed**: at 50,000 meaningful facts the high floor consumed **250,241 VM
steps**, versus **245** with no floor. At 10,000 facts the high floor consumed
50,241 steps; `009999` consumed 50,242 steps when the first survivor lay after a
long excluded prefix. The failing plan sought the partial meaningful index by
`task_id` only. This is a reproducible production-path work defect; the NULL
versus empty semantic test already passed and must remain intact.

The working repair extracts the exact production INSERT as `QUERY_FACT_REFRESH`
and splits NULL-floor EXISTS from non-NULL `f.ulid>?9` EXISTS using CASE, both
against `tracker_summary_meaningful`. The cost assertion covers histories 10,
10,000 and 50,000 with None, empty, middle and high floors, requires fewer than
1,000 VM steps and a plan containing the ULID range, and verifies expected
attention membership. `/tmp/task900-refresh-cost-green.log` now records **2/2
passed, exit 0** (owner-confirmed): all 12 combinations used **247–250 VM
steps**, and EXPLAIN includes `(task_id=? AND ulid>?)`. This supersedes the RED
physical-work result for the measured statement; source commit `fa8a4ff9b` is frozen; broader gates and independent review
remain pending.

### Filter-choice aliases and round-trip membership

At `484537214`, `sql/choices.rs::desired` read labels/status from display record
fields (lines 63–76), and `assignee` read display assignee (lines 90 onward).
The already-repaired list selector used exact canonical cells, so a shortened
choice could name a different task group or fail assignee validation. Choices
are part of the predicate contract: offered values must round-trip through the
same canonical selector, without aliasing a NUL prefix/suffix identity.

The working repair reads bounded canonical `labels_json`, status, assignee
kind/ID and `pending_present` from `tracker_task_query_sources`, preserving the
existing choice refcount delta machinery and invalid-label/source refusals.
Pending-only tasks produce the scalar `"any"` choice rather than inventing exact
target-agent membership or the `"none"` choice. Task/metadata producer order must
refresh canonical pending facts before choices, and repair/rebuild must remove
old materialized prefix aliases without rewriting canonical task records.

One additional resource edge accompanies exact IDs: a legal opaque agent ID of
128 bytes can expand during JSON serialization (for example NUL requires an
escape). The previous `list_task_filter_choices` key cap was 512 bytes at
`task_summaries.rs:241,258`. The exact serialized assignee-object key and opaque
cursor position now use a **1,024-byte bounded assignee-key/position admission**;
this is not permission to widen the 128-byte ID domain or silently shorten an
escaped key. Other choice keys remain capped at 512 bytes, values at 1,024
bytes, and the complete encoded cursor retains its unchanged 2,048-byte cap.
`BoundedPage::position` admits the complete authenticated position up to 1,024
bytes; `list_task_filter_choices` applies the larger key bound only to assignee.

The extra boundary regression
`maximum_opaque_agent_choice_pages_and_round_trips_without_restricting_ids`
first failed against the old 512-byte key bound: **0 passed, 1 failed, exit 101**,
`/tmp/task900-choice-key-bound-red.log`. The repaired final choice RPC group
passed **5/5, exit 0**, `/tmp/task900-choice-key-bound-green.log` (owner-confirmed).
Its actual three limit-1 pages contain a 127-NUL-byte ID, a 128-NUL-byte ID and
`zzzz`, with exact selection for each. Complete serialized key/cursor sizes are:

| Raw ID bytes | Serialized key bytes | Complete encoded next-cursor bytes |
| --- | --- | --- |
| 127 | 792 | 1,303 |
| 128 | 798 | 1,312 |
| 4 (`zzzz`) | 34 | No continuation |

Both nonterminal cursors are below the unchanged 2,048-byte complete-cursor cap.
These are measured encoded cursors, not estimates of an unencoded position.

Observed new tests against pre-fix producers:

- `/tmp/task900-choice-rpc-red.log`: **1 passed, 3 failed**. The existing
  normalized-status vocabulary/validation case passed; exact labels, exact
  assignee choices and pending-only Any cases failed.
- `/tmp/task900-choice-store-red.log`: **0 passed, 2 failed**, including stale
  prefix-choice rebuild and reassignment/deletion behavior.

Exact new regression names, inspected in the working source:

| Scope | Regression | Current evidence |
| --- | --- | --- |
| Actual production SQL work | `query_projection::cost_tests::actual_query_refresh_meaningful_floor_is_an_index_range_with_bounded_vm_steps` | RED above; final focused cost run 2/2, exit 0; 247–250 steps |
| NULL versus empty dismissal | `query_projection::cost_tests::actual_query_refresh_distinguishes_null_floor_from_explicit_empty_floor` | Passed in initial run and final focused cost run 2/2, exit 0 |
| RPC labels | `tracker_query_choice_roundtrip::label_choices_round_trip_nul_suffix_prefix_and_normal_counterparts` | RED above; RPC 4/4 or store 2/2 GREEN as applicable, exits 0 |
| RPC assignee choices | `tracker_query_choice_roundtrip::assignee_choices_round_trip_full_opaque_ids_without_prefix_aliases` | RPC 4/4 GREEN, then boundary-inclusive RPC 5/5 GREEN, exits 0 |
| Existing status vocabulary | `tracker_query_choice_roundtrip::status_choices_keep_existing_normalized_vocabulary_and_validation` | Passed in initial and final four-case RPC runs, exit 0 |
| Pending-only Any lifecycle | `tracker_query_choice_roundtrip::pending_only_any_choice_disappears_on_cancel_without_becoming_exact_target_membership` | RED above; RPC 4/4 or store 2/2 GREEN as applicable, exits 0 |
| Store canonical choice deltas | `query_choices::canonical_choices_preserve_opaque_labels_and_assignees_after_reassignment` | RED above; RPC 4/4 or store 2/2 GREEN as applicable, exits 0 |
| Stale-choice rebuild/canonical bytes | `query_choices::canonical_choice_upgrade_removes_old_prefix_aliases_and_preserves_status_validation` | RED above; RPC 4/4 or store 2/2 GREEN as applicable, exits 0 |

Completed focused logs are `/tmp/task900-choice-rpc-green.log` (**4 passed, 0
failed**) and `/tmp/task900-choice-store-green.log` (**2 passed, 0 failed**), both
with owner-confirmed exit **0**. These precede the additional worst-case escaped
key regression. The later boundary-inclusive RPC run above separately verifies
that larger-key path. The audit-writing subagent did not execute these focused tests; committed-head gate executions are recorded separately in the validation ledger.

The final frozen-source focused reruns are owner-confirmed exit **0**:
`/tmp/task900-choice-store-final-green.log` (**2/2**),
`/tmp/task900-refresh-cost-final-green.log` (**2/2**, again 247–250 VM steps),
and `/tmp/task900-choice-invalid-final-green.log` (**1/1**,
`invalid_filter_choices_fail_before_returning_an_unusable_page`). The
boundary-inclusive RPC log above is **5/5**. These outcomes belong to the
integrated working source subsequently frozen in `fa8a4ff9b`; they are not a
clean-head complete-suite or fresh-review claim. Fingerprint ownership already
includes the changed choices, maintenance and query-projection definitions;
the stale-alias reopen case passes with canonical record bytes unchanged.

### Pending-only Any consumer boundary

The source-backed consumer audit found no active shipped caller consuming
`tasks.filterChoices` in fetched SPA menus. The active
`spa/src/core/trackerTasksPane.js` menus use `filterOptions(state.all)` (line 284)
and dispatch `tasks.list` (lines 459/482). Shipped `sqlReadCredits` references are
an allowlist, not a fetch consumer. The frozen #891 `taskFilterChoices` reader is
exported but has no production source importer; self/tests do not establish
runtime activation. This is a source comparison, not a claim that existing
legacy paint semantics already match pending assignment.

The local Node witness `/tmp/task900-choice-ui-witness.log` records an actual
`"Anyone assigned"` option whose value and dispatch are `"any"`, and accepted
#891 query identity `assignees:["any"]`. It also records the incompatibility:
legacy client-side paint produced `paintAny:[]` and `paintNone:["pending"]`.
The lead authorized the canonical pending-only Any backend direction after this
call-graph/witness evidence; no silent legacy-helper parity or consumer
activation is inferred. The explicit #891 activation gap is
#891/c/tc-01M64A9D058M2B7EMP3N30G6YM. It requires consumer integration evidence
and review for the visible difference before enabling the bounded path.

| Additional source-to-predicate dependency | Repair boundary | Focused-stage verification |
| --- | --- | --- |
| Canonical dismissal suffix → meaningful-history EXISTS | Separate NULL/no-floor branch from indexed non-NULL ULID range in the actual production INSERT; preserve canonical empty-floor behavior | Production-path RED and focused GREEN recorded; source `fa8a4ff9b` frozen; final gates and Astra disposition recorded separately |
| Canonical labels/status/assignee → materialized filter choices | Read bounded structural source cells, not display strings; every offered value round-trips through canonical list matching | RPC/store and escaped-key RED/GREEN recorded; source `fa8a4ff9b` frozen; final gates and Astra disposition recorded separately |
| Canonical pending presence → Any/None choice | Refresh choices after source metadata, on cancel/rearm/repair/reassignment/deletion; no target-agent membership | Backend direction authorized; focused lifecycle regression passed; #891 activation review pending |
| Legal 128-byte opaque ID → serialized choice key/position | Bound complete JSON key and opaque position at 1,024 bytes without shrinking/capping the ID | Boundary-inclusive RPC 5/5 passed; full cursor cap unchanged; source `fa8a4ff9b` frozen; descendant execution heads are in the validation ledger |

The table retains focused-stage evidence. The [validation ledger](task-query-validation.md) pins the completed full run to `34dc`, affected helper-refactor tests to `f85`, and lint/release/cost samples to `557`. Final independent dispositions belong on #900; this map does not self-approve either Astra finding. The [cost comparison](task-query-cost/README.md) retains added costs and unmeasured domains for a separate cost disposition.

## Source-to-predicate map

All line references in this table are at `0d3e81643`, relative to `bridge/src/`.
“Safe by type/source” records inspected structural guarantees, not an additional
runtime test claim.

| Canonical source and inspected path | Affected new predicate | Finding / safe inputs | Intended repair boundary; verification status |
| --- | --- | --- | --- |
| `Task.updated_at:String`; `store/task_summaries/maintenance.rs::task_written_unchecked` binds the full scalar; `sql.rs:152` caps display; `query_projection.rs:156` reparses it | Updated order; final anchor fallback | NUL and overlong valid prefixes must remain Unsupported rather than become Known/Unknown. Missing/null/empty retain the declared Unknown behavior where applicable. | `query_sources::task_written` captures ≤38-byte source or an Unsupported sentinel; `query_projection::refresh` parses retained cells. History/metadata/dependency refreshes reuse them. **Focused source/RPC evidence below passed.** |
| `Task.created_at:String`; `sql.rs:152`; independently `sql/activity.rs:10`; `query_projection.rs::anchor` | Anchor fallback; Done fallback | Only Unknown created input may fall through to updated. Unsupported nonempty created input cannot silently fall through. | `query_sources::task_written` replaces canonical cells on every supported task change; `query_projection::anchor` consumes created classification independently of display/context strings. **Focused source/RPC evidence below passed.** |
| `Task.done_at:Option<String>`; `sql.rs:154`; `sql/activity.rs:9,24–26` | Inclusive `done_since` cutoff | Done chooses input if present, otherwise selected move event, otherwise created. Empty present input is Unknown and still wins source selection. Unsupported selected input cannot fall through. | `query_sources::task_written` retains input presence and bounded source; `query_sources::done` checks canonical `status_done` before selecting input/event/created. **Focused source/RPC evidence below passed.** |
| Canonical move-event `at`; `sql.rs::fact_rows` at `98–101`; `sql/activity.rs:24–26` | Event-derived Done cutoff | The selected fact is ordered by canonical `at DESC,id DESC`; classification must describe that same winner. | `query_sources::done` selects the same indexed fact winner with `typeof(at)` and BLOB-byte-length guards before returning ≤38 bytes. **Focused source/RPC evidence below passed.** |
| Retained session pair; `sql/activity.rs::full`, `append_update`, `patch_card`; `query_projection.rs::anchor` | Anchor order | A pair of safe integers with start ≤ last directly supplies the anchor. This numeric branch bypasses created/updated fallback only for anchor. Updated order independently checks updated input. | `query_projection::anchor` retains safe-pair precedence; `query_source_done_event_fallback_and_valid_session_bypass_use_selected_sources` covers bypass versus independent updated refusal. **Passed in the 11-case source run below.** |
| `Task.assignee` typed enum plus exact agent-ID String; `sql.rs:115–118`; `query_projection.rs:189–191`; app `tracker/selections.rs::matches_row`, `query_selections.rs::matches_row` | Agent bucket; positive assignee; exact assignee exclusion; held | Kind is typed. IDs may contain NUL within existing byte bounds. Empty project-bucket sentinel must not alias an agent ID. | `query_sources::task_written` captures kind/exact bounded ID; `query_projection::refresh` uses it for contribution keys, with an empty-agent guard; `CanonicalSelection::apply` and `task_query_matches` use exact matching cells. **Focused source/RPC evidence below passed.** |
| `Task.status:String`; `sql.rs:124`, `sql/activity.rs:9`; `query_projection.rs:158,183,192`; `tracker/selections.rs::matches_row` | Unfinished/counts; attention; history/static stream selectors; Done cutoff | Unlike state/priority, status is an arbitrary String in the stored Task. `done\u0000suffix` must not become Done; `in_review\u0000suffix` must not become In review. | `query_sources::task_written` derives unfinished/In-review/Done from full canonical status. Supported bounded status remains exact; overlong display becomes a nonmatching sentinel with `status_truncated`, exposed by `CanonicalSelection::apply` as `summary_truncated:true`. **Focused source/RPC evidence below passed.** |
| `Task.labels:Vec<String>`; `sql.rs:125–126,156–157`; `tracker/selections.rs::matches_row` | Positive label selection composed with new selectors | `api\u0000suffix` can fit 40 bytes and be shortened to `api`. Existing oversized/invalid-label flags do not establish NUL-safe equality. | `query_sources::task_written` retains bounded `labels_json`; `CanonicalSelection::apply` restores exact labels before selection. Existing invalid-label refusal still owns unsupported count/size/shape. **Focused source/RPC evidence below passed.** |
| `Task.read_through`, `watch_started_after`, `dismissed_through`: optional Strings; `sql.rs:135–142`; `sql/activity.rs:5–6`; `query_projection.rs:184–186` | Watched unread totals; unread asks/comments; legacy attention dismissal | Canonical `tracker/summary.rs::when` uses `split_once('-')` and preserves the suffix. Text `substr` can lose NUL before/inside the suffix. Native `app/tracker/tracking.rs:122–135` stores raw `event_id`. | `sql::opaque_suffix` uses BLOB first-hyphen slicing; `sql/activity::bounded_marker` preserves NULL versus empty; `query_sources::task_written` retains raw bounded marks/dismissed suffix and invalid-marker flag. `ensure_supported_markers` and indexed project preflight refuse overlong count/attention dependencies. **Focused source/RPC evidence below passed.** |
| Canonical comment/event ID String; `sql.rs::fact_rows` at `98` | Unread, mention and meaningful-after-dismissal comparisons | Typed record IDs can carry nonstandard bytes in imported records. Their suffix derivation must match `tracker/summary.rs::when`; capped display IDs are not ordering authority. | `sql::fact_rows` calls `sql::opaque_suffix` for canonical IDs; retained indexed suffix comparisons preserve full first-hyphen suffix. **Focused source/RPC evidence below passed.** |
| `Task.watched:bool`; exact canonical JSON boolean at `sql.rs:155`; retained activity context | Watched selection; unread and attention gates | **Safe by type/source:** supported typed producers cannot supply an arbitrary string as watched. This does not make marker comparisons safe. | Keep typed boolean and exact marker-derived counters separate. Covered by the source marker matrix and `query_attention::attention_distinguishes_agent_traffic_requests_read_and_watch_floors` in the 79-case run below. |
| Dependency projection from canonical task state/status and dependency endpoints; `sql.rs::dependencies`, `refresh_dependencies`; `query_projection.rs:193–194` | Blocked; user-ready attention; agent static streams | **Safe by source:** dependency SQL compares canonical task scalar columns and emits a boolean. It does not derive blockers from display status. | `producer_transactions::native_blocker_lifecycle_refreshes_counterparts_and_rolls_back_on_projection_failure` and `query_attention::user_blocking_review_and_finished_reasons_match_source` passed in the 79-case run below. |
| Canonical delivery-intent state and target; `sql.rs::enrich_patch`, `refresh_metadata`; `query_projection.rs:187,195` | Undelivered; held; pending-assignment positive selectors | State membership is checked before display, with provisioning displayed as queued. Target is arbitrary JSON in `DeliveryIntent`; display `target.kind`/agent ID are capped. Leading NUL in a nonempty kind can change held. | `query_sources::metadata_refresh` retains exact pending-state existence and JavaScript kind-truthiness booleans. `query_projection::refresh` consumes them; metadata output preserves bounded target IDs without NUL-prefix aliases. **Focused source/RPC evidence below passed.** |
| `ReviewMode` enum/default; `query_projection.rs::refresh_review_mode` reads canonical review JSON | `exclude_review_mode` | **Safe by type/source:** supported typed modes are snapshot/pull_request; missing historical mode defaults to snapshot. No summary display field is used for this predicate. | `query_projection::snapshot_review_default_and_rebuild_keep_exact_counts` and `producer_reviews::native_pr_publication_opinion_and_projection_failure_are_atomic` passed in the 79-case run below. |
| `TaskState` enum and `TaskPriority` enum; `tracker.rs:98,126`; `sql.rs:124,127`; `query_projection.rs:180,192` | State filters; priority order; unfinished | **Safe by type/source:** supported typed producers serialize fixed enum strings. Arbitrary raw SQL corruption is outside automatic producer guarantees. Canonical status remains a separate unsafe String input. | Safe-input guarantee is inspection of typed serialization, not an extra arbitrary-input runtime test; enum semantics are unchanged. |
| `Task.links.workspace_ids` and `Task.trackers`; `query_projection.rs::refresh_relations` | Linked-workspace/tracker membership and provenance | **Safe by source:** exact typed canonical vectors populate independent membership rows; capped display arrays do not determine membership. | `query_projection::query_projection_counts_memberships_and_generation_follow_transaction` and `query_projection::imported_project_reparent_moves_counts_and_all_relation_keys` passed in the 79-case run below. |
| Canonical `TaskEventKind`, `Actor`, comment author and boolean ask fields; full canonical JSON `payload.by`; `sql.rs::fact_rows` | Meaningful/activity/unread/ask classification | **Safe by type/source:** these predicates compare canonical fields before display; enum kinds/actors and boolean asks are typed. `payload.by` equality uses the full canonical JSON value. Fact-ID/marker ordering remains a separate dependency. | `query_attention::dismissal_ignores_bookkeeping_until_meaningful_workspace_activity` and `query_attention::imported_unknown_event_notify_flag_does_not_create_a_canonical_ask` passed in the 79-case run below. |

## Bounded read and maintenance boundary

The repair must not move canonical task-body parsing, full relation decoding or
history replay into a query. Summary pages continue through indexed order ranges
and bounded candidate admission; per-candidate matching uses bounded structural
rows and exact relation seeks. Counts remain materialized bucket point reads;
lookup remains a project-number point read. Unsupported-source preflight must be
an indexed maintained fact, not a project drain.

Task writes already possess the typed task and can retain small structural cells
before lossy display serialization. Ordinary history maintenance must reuse
those cells plus bounded retained-fact seeks; it must not reopen a canonical task
body to recover facts discarded earlier. A selected Done event can use the
existing indexed `done_move` winner. Exact suffix extraction must preserve
canonical bytes; a BLOB-safe slice can avoid SQLite text-NUL truncation without
restricting which otherwise supported ID characters are legal.

Overlong inputs are not permission to fabricate a bounded prefix as an exact
identity, marker, status, label or timestamp. Existing declared resource domains
and explicit refusal reasons must govern them. Any additional domain boundary
needs to be recorded with its affected primitive and evidence. There is no new
search schema, matching capability or claimed bounded search conversion here.

## Consumer integration gap: opaque marker presence

The backend target is additive `opaque-marker-v1` semantics from the canonical
Rust source. It distinguishes absent markers from present empty markers.
`bridge/src/tracker/summary.rs:89–102` uses `Option::or_else`: `None` permits the
latest Watched-event fallback, while `Some("")` supplies an explicit empty
baseline and bypasses that fallback. `summary.rs:162–169` compares the complete
suffix after the first hyphen without a prefix-regex restriction.

This does **not** match every existing SPA helper. The primary integration
regression requirement is #891/c/tc-01M647J3K88WM89J6KE1KQ77PM; the earlier
#891/c/tc-01M647EAW69ZYAKA845QVCQG29 records the initial incompatibility against
frozen foundation checkpoint
`5a04b2658204ad794fb97910e9412d88aa0aca86`, which remains unchanged by this audit:

- `spa/src/core/taskSummaryCache.js::preserveMarks` at lines 66–77 preserves a
  selected marker only when truthy; `coversMarks` at lines 83–84 compares against
  `incoming[field] || null`. `strongerMarks` at lines 81–82 also gates on
  truthiness.
- `spa/src/core/trackerUnread.js::latestTaskMark` at lines 6–15 uses the existing
  `tc`/`te` nonempty suffix regex through `taskUnreadKey`; empty markers become
  null. `taskWatchBaseline` at lines 20–24 uses `if (persisted)` and therefore
  falls back to Watched events for an empty persisted marker.

A second source difference concerns suffix extraction and comparison.
`tracker/summary.rs::when` keeps the entire suffix after the **first** hyphen, or
the whole string if no hyphen exists. The frozen SPA's
`trackerUnread.js::taskUnreadKey` accepts only `tc`/`te` IDs with a nonempty suffix
that contains no further hyphen. Separately,
`spa/src/core/trackerAttentionModel.js::orderedId` at line 45 uses
`split("-", 2).at(-1)`, discarding later segments of a multi-hyphen ID. JavaScript
string comparison also uses UTF-16 ordering, whereas canonical Rust string
ordering follows UTF-8 bytes. Existing ASCII ULID examples do not prove parity
for opaque Unicode strings. Do not validate the additive profile solely against
one of these incompatible consumer comparators.

The source comparison exposes a compatibility boundary, not authority to change
SPA behavior silently. The lead selected canonical Some-empty/None semantics
for the additive backend profile and was informed of the current-consumer
incompatibility. Backend profile pinning does not authorize an unexamined
consumer-visible change and is not a claim of parity with these legacy helpers.

Before selector activation, #891 must supply regressions for absent versus
present-empty markers, NUL, whitespace, Unicode/UTF-8, no-hyphen and multi-hyphen
markers, including marker floors, ordering and coverage; compare watched/unread
behavior against **both** the frozen consumer and canonical Rust source; and
obtain an explicit review disposition for any visible differences. The
foundation checkpoint is not updated by this document. Backend executable
canonical-versus-query empty-marker and first-hyphen/full-suffix cases passed;
exact names and backend results are recorded below. Consumer behavior/review remains
pending. Search and other
legacy compatibility exclusions remain unchanged.

## Shared repair and evidence checklist

The same canonical derivation must serve native task/history/metadata producers,
explicit import/repair and startup rebuild. A source change that leaves the same
capped display prefix still needs to refresh structural facts, counts and basis.
Rollback must restore old contributions and generation together; reassignment,
reparenting and deletion must retire the old contribution. Reopening after an
intact repair must preserve the correct facts.

Inspected fingerprint ownership includes `query_sources.rs`;
`tracker_task_query_sources` is owned by the query TABLES/schema manifest and
`reset_projection` table list. Producer wiring captures source cells before
display in `maintenance::task_written_unchecked`, refreshes canonical pending
booleans on task/metadata writes, and reuses retained sources on history and
dependency updates.

Fingerprint/manifest ownership includes the new structural source schema and
producer definitions. A persisted projection built from the lossy definitions
must rebuild through bounded reset/backfill before read-pool admission; marking
only an in-memory fast path fixed is insufficient. Existing migration ordering
rebuilds history facts before task reductions. Final source verification must
confirm any new module is included in the definition fingerprint and table
reset/manifest ownership.

Exact names below are relative to
`store::tests::task_summaries::query_timestamp_sources` (store cases) and
`app::tests::tracker_query_exact_text` (RPC cases). Names were checked against the
working source; results come from the completed logs and owner-confirmed exits
below, not from the tests' presence.

| Required regression category | Exact focused cases | Verified evidence |
| --- | --- | --- |
| Canonical timestamp NUL/long prefix; updated/created/input-Done/event-Done; safe session bypass | Store `query_source_timestamp_bytes_reject_nuls_and_overlong_valid_prefixes`; `query_source_project_gate_cannot_skip_bad_old_excluded_rows_and_clears_after_correction`; `query_source_done_event_fallback_and_valid_session_bypass_use_selected_sources` | Passed: source 11/11 and/or RPC 6/6 as applicable; exits 0 |
| Exact assignee buckets, positive/exclusion, pending target and creator identity with NUL | Store `query_source_exact_agent_buckets_preserve_nul_prefix_and_suffix_through_history_and_reopen`; RPC `nul_prefix_and_suffix_assignees_keep_exact_counts_positive_groups_exclusions_and_repair`; `pending_target_output_preserves_exact_assignee_id_without_inventing_membership`; `created_by_and_identity_dictionary_keep_structural_nul_ids_exact` | Passed: source 11/11 and/or RPC 6/6 as applicable; exits 0 |
| Canonical status and labels versus capped display | Store `query_source_status_done_prefix_is_not_completion_and_labels_remain_exact`; RPC `status_and_label_nul_suffixes_do_not_join_ordinary_static_groups` | Passed: source 11/11 and/or RPC 6/6 as applicable; exits 0 |
| Read/watch/dismiss and retained fact-ID suffixes | Store `query_source_read_watch_dismiss_and_fact_suffixes_preserve_nul_bytes`; `query_source_marker_profile_matches_canonical_first_hyphen_without_truthy_normalization`; RPC `watch_floor_missing_and_explicit_empty_remain_distinct_in_actual_counts` | Passed: source 11/11 and/or RPC 6/6 as applicable; exits 0 |
| Marker UTF-8 byte boundary, project-wide scope, read-only structured refusal and recovery | Store `query_source_marker_domain_uses_utf8_bytes_and_project_scope_and_clears_after_repair`; RPC `marker_byte_domain_refusals_are_project_wide_nonretryable_and_read_only` | Passed: source 11/11 and/or RPC 6/6 as applicable; exits 0 |
| Held/undelivered canonical target/state composition | Store `query_source_pending_kind_truthiness_preserves_leading_nul`; RPC pending-target case above | Passed: source 11/11 and/or RPC 6/6 as applicable; exits 0 |
| Shared stale-definition restart/reopen and canonical bytes unchanged across repaired layers | Store `query_source_old_prefix_projection_rebuilds_all_layers_without_canonical_rewrites` | Passed: source 11/11 and/or RPC 6/6 as applicable; exits 0 |
| Atomic reassignment, invalid-source rollback, correction, deletion and no empty-agent project-bucket alias | Store `query_source_reassignment_marker_failure_rolls_back_and_empty_agent_cannot_duplicate_project`; assignee RPC case above | Passed: source 11/11 and/or RPC 6/6 as applicable; exits 0 |
| Bounded projection-only queries and no canonical-body/history scan in ordinary history maintenance | Inspected `query_sources::load/done`, `query_reads::task_summary_by_number`, partial-index validity seeks, `maintenance::history_append`; earlier producer poisoned-body regression remains separate evidence | Source inspected; focused source/RPC runs passed; broader gates separately attributed |
| Consumer opaque-marker-v1 activation: absent/empty/NUL/whitespace/Unicode/multi-hyphen floors/order/coverage; frozen-consumer versus canonical behavior | Primary #891/c/tc-01M647J3K88WM89J6KE1KQ77PM; earlier #891/c/tc-01M647EAW69ZYAKA845QVCQG29; checkpoint `5a04b2658204ad794fb97910e9412d88aa0aca86` unchanged | Pending #891 integration and review before activation |
| Fresh independent scope/code/Astra reviews at repaired exact head | Parent review coordination | Pending |

Observed intermediate evidence is retained rather than relabeled as success:
`/tmp/task900-all-source-red.log` records the initial eight store regressions
failing; `/tmp/task900-query-exact-text-red.log` records the initial two RPC
regressions failing. A subsequent combined attempt in
`/tmp/task900-all-source-green.log` reached 10 store passes and one failure in
`query_source_marker_profile_matches_canonical_first_hyphen_without_truthy_normalization`
for `read None` (query unread 1 versus canonical 2). Its filename is not a passing
result (exit 101). A root-owned read-only SQLite reproduction isolated empty-BLOB slicing
returning NULL in this helper path, collapsing an explicit empty watch baseline
into absent/fallback behavior; the helper requires an explicit zero-byte branch.
A follow-up compile at `/tmp/task900-all-source-green2.log` was intentionally
cancelled with exit **143** before test execution to add the trailing-hyphen
empty-suffix case; it is not a pass. The final helper explicitly separates NULL
input, zero-byte input, and an empty suffix after the first hyphen.
The later profile/upgrade/rollback cases have test-first source, but no
separately observed initial-red claim is made for those cases. Final repaired
outcomes below supersede the failed attempt without erasing it.

The canonical marker profile store case runs each of read/watch/dismiss through
None, empty, whitespace/tab, NUL, Unicode, no-hyphen, multi-hyphen and
trailing-hyphen/empty-suffix examples;
it compares query unread totals with canonical `unread_since_mark`, compares
legacy-attention membership with canonical `after` and bookkeeping predicates,
and verifies persisted marker presence/bytes. The RPC watch-floor case checks
None → empty → None yields totals 1 → 2 → 1 and lookup preserves null versus
empty. Both executable cases passed in the completed runs below; the backend
case does not substitute for #891's pending current-consumer comparison.


## Completed focused repair evidence

The production owner confirmed all three final process exits as **0**:

- `/tmp/task900-all-source-green3.log`: **11 passed, 0 failed**, all source
  regressions named above. Cargo emitted
  `bridge/target/debug/deps/build_bridge-0521f5ae737d1028` from the frozen
  integrated working-tree repair source.
- `/tmp/task900-all-source-rpc-green.log`: **6 passed, 0 failed**, all RPC
  exact-text regressions named above, run directly through that same emitted
  binary without another compilation.
- `/tmp/task900-all-source-projection-green.log`: **79 passed, 0 failed**, the
  complete `store::tests::task_summaries` projection group through the same
  emitted binary, including the 11 new source cases. This is a group run, not
  the complete bridge suite.

The emitted binary SHA-256 is
`edf81e7cb31b3a9314d52e4e3e3cd3e3e914e27e6f21268df346633d9095a1fe`. The
compiled integrated production/test working tree was subsequently committed in
`eca9e4692` and `f2769d34ddf9cfb9614a6b2d475b6b2f992d7f32`; it was not
rebuilt as a clean-head whole-suite run for these focused results.

The source run includes canonical-versus-query comparisons for every marker
case and the shared startup rebuild test. The latter verifies rebuilt
structural sources, query facts, aggregate buckets, retained activity context,
display unread values and timestamp-invalid flags while canonical task/comment
record strings remain byte-for-byte unchanged. The rollback case verifies that
a projection failure preserves canonical task data, old bucket contribution,
invalid-marker state and basis, then checks successful correction and deletion.

These are working-tree focused outcomes, not an assertion that the complete
suite ran on a subsequently named clean commit. Additional gate attribution
belongs to the parent validation ledger (`task-query-validation.md`).
The document author inspected sources/logs and ran no tests for this audit.
The clean committed `cf06f73af8b94ee30d4671043fb35bc2a30cd930` full suite later
exited 0 (5,995 unit passes/24 ignored, 56 API contracts and all normal
integrations); code-identical `f2769` Clippy and `cf06` formatting/Semgrep passed.
Both fresh isolated release cohorts at `cf06` exited 0, including the summary
maximum 98.262 ms with only 1.738 ms observed margin. Exact attribution and
startup/write/memory/storage costs belong to that ledger and performance record.
Fresh independent scope/code/Astra dispositions are recorded on task #900;
neither P2 is self-approved by this audit, and #891 activation remains separate.

Earlier parser-only evidence remains in `task-query-timestamp-domain.md`; it does
not prove canonical source selection, aggregate maintenance or any of the
request-changes repairs above. Final validation must name the actual source head
and distinguish new focused regressions from earlier whole-suite evidence.


### Rendered filter-menu activation gate

#891/c/tc-01M64B6AD18F38Q7SYDE3YJVQZ pins an additional rendered-control
boundary identified by #891 in the current SPA:
`filterMenuControl`'s `rowHtml` applies `esc(value)` into a `data-value`
attribute. Mouse selection reads `dataset.value`, whereas Enter selection uses
the original `rows[active].value`. #891's isolated witness using the current
escaping helper and JSDOM observed a NUL in that attribute becoming U+FFFD.
This is source/witness evidence reported by #891, not a browser test executed
by the #900 audit author.

Consequently, the backend's exact RPC round-trip tests do not establish that
rendered mouse and keyboard selection preserve the same opaque value. Before
#891 activation, the actual rendered control must demonstrate mouse and Enter
round trips for NUL-bearing labels and opaque assignee values, including
prefix/suffix collision counterparts and a literal U+FFFD (`�`) value distinct
from NUL, and prove the selected value reaches the
canonical query unchanged. DOM escaping safety alone is not an equality proof
for attribute-backed identity transport.

This rendered-control check is an additional UI activation gate, alongside the
existing marker/source and pending-only Any compatibility gates. Backend
coverage does not satisfy it. No new matching semantics, ID restrictions,
legacy behavior change or consumer activation is authorized by this finding;
the backend exact semantics and activation hold remain unchanged. The rendered
control fix is already routed to #891 as authorized integration work. This
notice requires no backend change or backend gate rerun.

The authorized future #891 fix resolves the original choice through stable in-memory row identity/index, preserving display escaping. Mouse and Enter must dispatch identical original values; this implementation remains outside the frozen backend task.
