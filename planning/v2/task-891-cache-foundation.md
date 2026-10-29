# Task #891 cache foundation

This is independent groundwork for slice B, based on freshly fetched
`origin/dev` **12b8f869bd94e259666df69f1a8c0cde5c695982**. It is not a
runtime cutover or completion of #891. Existing task consumers and RPC paths
remain in place until the [missing-contract delivery split](task-891-contract-gaps.md)
is settled. No bridge API, bootstrap, journal, connection ring or consumer
behavior changes in this foundation.

## Ownership

All addresses use the existing device/project cache scope, so ordinary project
eviction reaches the new records. No cache schema bump or store is introduced.

| Kind / sub-key | Contents and authority |
| --- | --- |
| `tracker-summary` / task ID | One compact summary and its client read order. The newer read and task timestamp fence stale summary bodies. Existing PR-version floors are retained. |
| `tracker-summary-identity` / agent ID | Shared compact identity and read order, independent of task/detail duplication. |
| `tracker-summary-query` / canonical filter JSON | Loaded task IDs, identity IDs, refresh generation, last published generation, cursor, completion and refresh state. It contains no task bodies or invented total. |
| `tracker-summary-invalidation` / empty | Latest immediate-write read order. Fences all older project query tickets without enumerating inactive cached queries. |
| `tracker-summary-observation` / empty | Latest project page observation; conservatively marks older memberships, including empty ones, stale when another query can reveal entrants. |

`taskSummaryQuery.js` canonicalizes the currently negotiated summary filters
only: state/status/label/assignee selections, OR within each set and AND across
sets. It sorts and deduplicates equivalent sets, preserves opaque actor ID
case, and matches ASCII-only label folding and known status display aliases.
Empty UI sets omit the unrestricted dimension; scalar/plural conflicts and
unsettled selectors are refused. Future selectors and orderings must explicitly
extend this identity rather than collapsing into an unrelated existing query.

`taskSummaryCache.js` has no wire reader. A caller obtains a shared
`nextTaskRead()` token before dispatch, begins a query refresh, and lands one
page with its captured ticket and session/account acceptance check. The check
runs inside the same IndexedDB transaction as query, entity and identity
writes. Each continuation must match the query's current cursor. No loop
drains pages. An empty page with a different cursor advances normally;
malformed/nonadvancing replies cannot consume the old cursor. Failed reads
leave the last successful membership and cursor intact. A refresh preserves
the previous paint until its first successful page replaces membership.

Query replacement changes membership only. It never infers entity deletion
from absence. Independent filters/board columns hold separate cursors and
generations while sharing entity records. A later row in one query cannot be
overwritten by a delayed older page from another query.
Each membership also retains the read that observed its named row. A shared
row updated by a different query makes the earlier membership stale, rather
than presenting incompatible predicate membership as freshly established.
The #890-only foundation conservatively treats newer project observations as
stale evidence, including for empty memberships. This can require extra
first-page revalidation after unrelated query reads; it is not a performance
claim. #900's projection basis will let integration distinguish compatible
same-basis pages from actual membership changes. No durable event journal or
background sweep is introduced here.

Immediate action patches update the shared summary and invalidate older query
tickets atomically. They preserve omitted compact fields and reject detail
content from the summary projection. Read/watch/dismiss marker floors are
monotonic; older marker observations cannot restore unread counts. Full
`tracker-task` detail/timeline records remain separately owned and untouched.
Authoritative full-task action results have an explicit projection mode that
clears omitted false watching/completion/review/deferred fields; partial
optimistic patches retain omissions. Both accept a transaction-time lifetime
check. PR server versions reconcile independently of ordinary row read order.

## Integration still required

This module is not yet imported by production readers. The remaining work is
substantial: independently negotiated capability/support storage, first-page
read coordination and explicit continuation, per-column board controllers,
choice-page readers/menus, summary-aware list/attention rendering, immediate
mutation/rollback integration, authoritative filter exits/deletions and query
invalidation on pushes, counts/Inbox/reference/workspace/agent consumers, and
opened detail/history paging. Aggregate revision and local write reconciliation
must follow the settled backend contracts. Unknown membership cannot become
an empty set, and stale invalidated memberships must be refreshed/filtered by
the eventual controller before being presented as authoritative.

The current foundation invalidates all project queries conservatively on an
immediate summary write. It does not yet reconcile each query's predicate or
insert a newly created row into every loaded range. Those are consumer
integration responsibilities and must be tested before enabling it. Existing
legacy action/read fences and #879/#881 cleanup/read-state repairs stay in
place during that integration.

## Evidence

Tests were committed before implementation, with observed red runs for the
missing module, action/marker fences and detail projection. Focused tests use
the real local-cache transactions with fake IndexedDB. They cover canonical
queries, partial membership without totals, empty continuation, duplicate and
stale replies, failed refresh preservation, session refusal at commit,
independent board column cursors, newer summary/marker authority, immediate
action invalidation, shared identities and untouched opened details.

No user-visible UI change or performance improvement is claimed for this
unintegrated foundation. #869 fixtures still exercise the exact unchanged
baseline reader paths; final comparison and meaningful mounted UI evidence
remain required after integration. Full SPA tests remain the coordinator's
serial gate.
