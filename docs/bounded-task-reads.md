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
duplicates and normalized case do not change that binding. Clients must treat
cursors as opaque rather than parse, synthesize or persist their internals.

The response contains `project_id`, `summary:true`, `tasks`, a shared
`identities` dictionary, `user_session`, and optional `next_cursor`. Rows carry
task identity, number, title, state, status, labels, priority, assignee,
creator, creation/update/closure/Done times, watch and unread/activity summaries,
compact dependency counts, read/watch/dismissal cursors, and Needs you counters,
attachment count, bounded agent trackers and workspace/parent navigation references, and optional
review and deferred assignment summaries. `done_at` and `closed_at` use explicit
nulls when absent. `watched` is always present; unread/activity fields appear
only where applicable. Needs you counters count unread agent comments and mentions
after the read marker, preserving the existing attention semantics independently
of the watch-start floor used by `unread_count`. Rows carry no body, attachments, full repository or
conversation links, timeline, dependency expansions or identity dictionaries.
`tasks.get` remains the detail read.

Candidate work and serialized bytes are bounded independently of the requested
row count. Consequently a page may contain fewer than `limit` rows, including
an empty `tasks` array with a continuation. Keep reading until `next_cursor`
is absent. An oversized imported record still occupies its task position:
bounded display data carries `summary_truncated:true`, while filtering uses
the complete stored memberships. Do not interpret a truncated summary as the
complete task detail.

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

Selections are ORed within a dimension and ANDed across dimensions. Omit a
dimension for no restriction. Empty plural arrays, null plural selections,
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

Both reads use the existing SQL lane outside the app lock. Summary and choice
records are materialized on writes so serving a page does not decode each task's
history. The shared contract witnesses live in `fixtures/api/v1/tasks.list.json`
and `fixtures/api/v1/tasks.filterChoices.json`; historical legacy examples remain
alongside the new summary examples.
