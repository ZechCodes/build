# Bounded task reads

Wire 3.26.0 adds `tasks.listSummaries`, a capability selecting the summary mode
of the existing `tasks.list` RPC. It is not a new RPC name. The separate
`tasks.filterChoices` verb is advertised independently. These APIs do not switch
the SPA sync path; clients opt in only after checking the greeting.

## Summary pages

```json
{"project_id":"proj-1","summary":true,"labels":["bug","ui"],"limit":100}
```

The default limit is 100 and the maximum is 500. Limits must be positive
integers. `limit:null` and `cursor:null` are invalid in summary mode. Task order
is descending project task number. Pass the opaque `next_cursor` back as
`cursor` to continue with the same project and selections. A cursor is bound
to the store, project, summary mode and canonical selections; it cannot cross
devices, projects, modes or changed selections. Equivalent selection order,
duplicates, normalized label case and recognized status display aliases do not
change that binding. Agent IDs remain exact and case-sensitive. Clients treat
cursors as opaque rather than parse, synthesize or persist their internals.

The response contains `project_id`, `summary:true`, `tasks`, a shared
`identities` dictionary, `user_session`, and optional `next_cursor`. Rows carry
task identity, number, title, state, status, labels, priority, assignee,
creator, creation/update/closure/Done times, watch and unread/activity summaries,
compact dependency counts, read/watch/dismissal cursors, and Needs you counters,
attachment count, bounded agent trackers and workspace/parent navigation
references, and optional review and deferred assignment summaries. `done_at`
and `closed_at` use explicit nulls when absent. `watched` is always present; unread/activity fields appear
only where applicable. Needs you counters count unread agent comments and
mentions after the read marker, preserving the existing attention semantics
independently of the watch-start floor used by `unread_count`. Rows carry no
body, attachments, full repository or conversation links, timeline, dependency
expansions or identity dictionaries.
`tasks.get` remains the detail read.

Each serialized result is at most 1,048,576 bytes. The reader reserves 16,384
bytes for envelope metadata and limits each row plus its identity input to
65,536 bytes. A page scans at most `4 * limit` candidate rows plus one lookahead
(default 401, maximum 2,001), fetched in chunks of at most 32. A byte stop does
not decode every remaining candidate. The shared identity dictionary holds at
most 1,000 entries, using at most two creator/assignee references per accepted
row; it does not load a project's full agent roster.

These work and byte bounds may produce fewer than `limit` rows, including an
empty `tasks` array with a continuation. Keep reading until `next_cursor` is
absent. Display-only fields can be capped with `summary_truncated:true`: the
title to 1,024 Unicode characters, workspace navigation to 20 IDs, and tracker
lists to 32 references. Valid label sets retain all 20 labels of at most 40
UTF-8 bytes each. Opaque IDs retain their exact spelling. Invalid stored labels
or oversized opaque IDs produce explicit errors, as does an individual row
that exceeds the byte cap. These stored-data failures use `unavailable` with
`retryable:false`, no `result`, and no continuation. A task is never silently omitted or retried
forever at the same cursor. A truncated summary does not replace complete
task detail.

Calls with `summary` absent or false retain their legacy rows and existing
`tasks.listPaged` behavior, including an unbounded list when `limit` is absent.
The legacy response does not acquire `summary` or shared `identities` fields.

## Selections

| Dimension | Scalar | Multiple selections | Values |
| --- | --- | --- | --- |
| State | `state` | `states` | `open`, `closed` |
| Status | `status` | `statuses` | Column slugs or display names |
| Category | `label` | `labels` | Case-insensitive labels |
| Assignee | `assignee` | `assignees` | Existing assignee objects, `none`, `any` |

Each plural dimension holds 1 to 100 selections. Selections are ORed within a
dimension and ANDed across dimensions. Omit a dimension for no restriction. Empty plural arrays, null plural selections,
invalid members, and both scalar and plural keys for one dimension are
`invalid_params`; the latter includes a scalar null alongside a plural key.
Legacy scalar nulls by themselves retain their previous meaning. Status and
state are distinct: Done is a workflow status, not a task state.

## Filter choices

```json
{"project_id":"proj-1","dimension":"label","limit":100}
```

`dimension` is exactly `label`, `assignee`, `status` or `state`. Choices span
the project independently of current task filters or the loaded task page.
The default and maximum limits are 100 and 500. This read has its own opaque
cursor bound to store, project and dimension. Its result is
`{project_id,dimension,choices,next_cursor?}`. Choices are label strings,
status/state slugs, or assignee filter values. The string `none` represents
unassigned tasks. Empty choices pages with continuation are valid: continue
until `next_cursor` is absent. Display order is server ordering, not task order.
The choice query reads at most `limit + 1` distinct indexed values. A single
choice over 65,536 bytes produces an explicit error. Store reads additionally
reject choice values over 1,024 bytes, keys over 512 bytes, and imported label
choices over the valid 40-byte limit. Its serialized result has
the same 1,048,576-byte cap. Both APIs reject cursors over 2,048 encoded bytes;
the internal seek position is capped at 512 bytes.

Both reads use the existing SQL lane outside the app lock. Summary and choice
records are materialized on writes so serving a page does not decode each task's
history. Persisted projection checks cap each summary record at 32,768 bytes,
its identity input at 8,192 bytes and its assignment target at 1,024 bytes.
The shared contract witnesses live in `fixtures/api/v1/tasks.list.json`
and `fixtures/api/v1/tasks.filterChoices.json`; historical legacy examples remain
alongside the new summary examples.

## Projection lifecycle

Store startup builds or repairs projections before opening its reader pool and
before creating the shared app mutex. Startup time is separate from page latency;
the first page never queues a hidden projection rebuild. Backfill checkpoints
commit with batches of at most 128 canonical rows. A failed batch rolls back its
facts and checkpoint together; restart resumes after the last committed batch.
Completed projections retain a Rust/SQL-definition fingerprint, installed-object manifest
and SQLite schema generation. Intact reopens skip projection DDL, backfill and
eager maintenance-statement preparation. An unrelated schema change only revalidates the
manifest; missing indexes are restored selectively, without replaying history.
Missing/changed projection tables or maintenance definitions first persist
a repair marker, clear derived rows
in bounded batches, then restart the backfill. Canonical records remain intact.
Empty derived tables are recreated with the expected definitions in the same
transaction that clears obsolete checkpoints and the repair marker. A staged
definition fingerprint is saved before the first backfill batch, so an interrupted
migration cannot resume a committed prefix under changed projection rules.
Batch counts bound row cardinality, not the size of an imported canonical body;
one legacy record can still be large and must be parsed during migration.

Supported Rust producers maintain cards, compact activity facts, dependency
counterpart counts, bounded identities, and refcounted distinct choices inside
the canonical writer transaction. No projection triggers or helper views are
installed. An upgrade retires legacy projection programs transactionally before
backfill and reader admission. Maintenance finishes before the producer returns,
so subsequent statements in the same transaction observe the derived change;
a failed canonical write or maintenance step rolls both back. A chronological
history append updates retained counters and session times from the new fact.
History repair, late activity and watch-floor changes use the exact compact-fact
derivation. Page reads use neither path.

Independent SQL that bypasses these Rust producers does not automatically
maintain projections. Import/repair code must call the shared maintenance API
inside its transaction, refreshing both OLD and NEW endpoints for reparenting
and capturing neighbors before deletion cascades remove their edge rows. This
explicit ownership replaces the earlier independent-SQL guarantee by the design
decision on #887. QA consistency tests compare canonical data with projections;
future producer paths must participate in that coverage. Performance evidence
must report initial backfill, subsequent startup, first/warm writes and actual
carrier reads separately.

| Canonical producer | Maintenance ownership |
| --- | --- |
| `tracker::write_tracker_task` | Compact typed task/card/context, choices, own dependency counts; state/status changes refresh dependents. Unchanged saves skip derived work. |
| `tracker::append_activity` | One fact per inserted comment/event, incremental retained activity/card; duplicate IDs do not increment counts. |
| `reviews::write_header` | Review metadata on the same task. |
| `delivery_intents::write` and project cancellation | Deferred assignment metadata for affected task IDs. |
| `waiting_on::graph::edit` and reconciliation | Own card after timestamp changes and both dependency endpoints after edge edits. |
| `entities::write_agents`, owner writers and owner deletion | Bounded identity rows, including removed roster members and changed owner names. |
| `reset_conversation` | Identity refresh after replacing complete agent skeletons. Receipt acknowledgments/delivery-status and retention mutate only transcript fields, which do not enter the identity projection. |
| Project task-history deletion | Capture tasks and dependency neighbors before deletion; remove cards, facts and memberships, then refresh surviving counterpart counts. |
| Legacy import and startup migrations | Runtime import uses the common entity producers; task rename and old agent migrations precede projection backfill. Explicit import/repair hooks cover keyed edits, deletion and reparenting. |
