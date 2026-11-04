# Scoped sync journal

This is the bridge implementation for #892 and #995. The journal owns
transactional publication, cursor validation, ReadPool recovery and bounded
retention maintenance. Task recovery ships in wire 3.32; #995 completes the
device producers and leased recovery described below. The typed sync methods
also have POST query resources. Existing read/write replies, subscriptions and
pushes keep their existing behavior.

The approved plan and rulings are on #892. No SPA consumer or #925 retained
task-selection service is implemented here.

## Publication

Every `StoreWriter` transaction owns one `PublicationBatch` for the canonical
SQL transaction. `sync_transaction` also permits explicit staging. Producers
stage final bounded summary-visible values. The
batch replaces repeated values for the same entity and publishes once after
the source/derived writes succeed, before the same SQLite commit. The caller
receives no success if publication or commit fails. The low-level store hook
is available only inside the store modules; it must not be flushed separately
by individual producers inside a shared transaction.

The persisted fingerprint compares the final visible value, deletion state and
stream membership with the last publication. An unchanged save appends nothing.
Reparenting routes a change to both old and new project streams. Deletes retain
their entity identity, revision and former routing through the retained interval.
The current task API rejects project/number changes; sync preserves that rule.

Task producer hooks collect a deduplicated TEMP set on the sole writer
connection. The set, its initial comparison baselines, all derived projection
writes, the journal append and its cleanup share the source transaction. The
set rolls back on failure. It is not the durable non-SQL dirty path. Canonical
task edits, inserted activity and committed review headers force one task
invalidation even if a bounded card did not change. Metadata-only hooks compare
final bounded display and lossless predicate inputs; private delivery checkpoints
and unchanged saves publish nothing. Startup backfills precede writer/read-pool
admission and do not consume the transaction-local producer set.

The producer guard inventories literal and formatted SQL writers using the Rust
syntax tree, including referenced statement constants. New canonical writers or
dynamic table writers require classification. It also pins the publication
owners' calls through the shared projection hooks and writer finalizer. Store
producer tests exercise the semantic and rollback boundaries; the inventory
alone does not prove atomicity.

Only committed summary-visible fields participate. Partial/streaming item
updates and ephemeral runtime facts must never enter a batch. Agent `working`,
running/liveness `state` and liveness-derived `available` keep their existing
push protocols. The committed-conversation amplification regression pins these
producer boundaries with a streaming fixture; accumulator coverage alone does
not establish producer coverage.

The journal and source share the store's existing WAL durability settings.
This preserves their process-crash atomicity without changing user-write ack
timing or adding an await. It does not strengthen the existing power-loss policy.

## Cursors and the head reader

`sync_incarnation` and `sync_cursor_key` are separate from the command journal
incarnation and task-query cursor key. Sequences are nonnegative signed SQLite
integers, represented on the wire by canonical decimal strings, including above
JavaScript's exact integer domain.

A bounded version-1 token uses HMAC-SHA256 with a random 32-byte persisted key.
It binds purpose, incarnation, exact normalized scope digest and sequence.
The scope includes the server-owned paired-device principal; callers cannot
provide one. Task filters must use the shared query normalization before creating
a scope. Opaque actor bytes are never clipped or case folded.

Malformed, unauthenticated same-incarnation, cross-scope and future tokens are
explicit errors. A structurally valid foreign incarnation requests a scoped
reset: a foreign store has a different secret, and that path cannot grant any
cursor progress. A retained-history gap requests `retention_expired` rather than
returning a replacement success cursor. A head reply is observational and does
not issue an applied-data cursor.

`ReadPool::acquire_sync` holds the fixed two-permit `SyncRecovery` quota and a
read-only connection. Its head read captures identity, head and retention proof
in one short read transaction, without the app lock or writer connection. The
execution adapter supplies the bridge epoch in every reply. Ordinary resume
cursors do not bind the process epoch; device leases will.

Current-row replay is idempotent by stable event sequence/entity/operation plus
the current entity revision. Retries may carry a newer row or membership. A
consumer applies a row only if its revision is at least its cached revision and
retains deletion tombstones by revision. With no intervening write, the same
bound run returns the same records. No retained replay-result service is planned.
The C4 wire fixtures must pin these rules for #893.

## Task snapshots and recovery

`TaskSyncScope` is constructed from captured server authority and the shared
#890 `TaskSelections`/`QuerySelections`. The digest includes canonical project
identity/key, normalized selection sets, exact opaque actor bytes, order and
query predicates. Page size is independent of scope identity. Task scopes bind
the stable task id. The adapter supplies the captured project directory and
the process epoch; these reads have no application reference.

Every data operation runs in one `SyncRecovery` read transaction. A project
snapshot captures W with its first page and uses signed live keysets on later
pages. It scans at most 128 candidates per page, including sparse predicates.
An empty page can have a next page. Changing order keys can produce repeats or
move rows between pages; replay after W supplies their current state. Persist
the snapshot cursor only after replacing the complete scope. Task snapshots
contain one bounded header and the newest 50 timeline entries, with an explicit
`has_more` flag; detail/history APIs remain responsible for older content.

All task summaries reuse `TaskSummaryView` and the maintained query projection.
The separate `selection.pending_present` comes from canonical SQL facts, never
the displayed deferred target kind. Leading-NUL display loss, absent/null/empty
targets and exact actor ids remain distinct. Identity names/directories belong
to independently synchronized device records; task rows contain actor ids and
do not import live identity overlays.

A changes run captures fixed H, fetches at most 501 routed events to enforce
the 500-event budget and returns batches of at most 100. Each continuation binds
the original cursor, H, applied position, event count and accumulated encoded
response bytes. The 1 MiB budget includes every returned envelope, including
continuation tokens. Fixed-width accounting avoids a self-referential token
length. Every page rechecks retained history; losing an in-flight interval
requests a reset without issuing a success cursor.

The event sequence and operation describe the durable event. The row, membership
and revision describe current state and can be newer than H. A filter exit
returns `member:false` with its current row. A missing/deleted entity returns a
null row and its current tombstone revision, even when replaying an older upsert
event. Apply current state only at a revision at least as new as the cached
revision; keep entity and scope-membership tombstones so redelivery cannot
resurrect older data. No replay-result state is retained between requests.

## Retention

The named production limits are seven days, 100,000 entries and 64 MiB of
accounted journal metadata. Each metadata entry accounts for encoded identity,
operation and routing plus fixed envelope overhead. SQLite page/index/free-page
space is separate from that payload counter and remains owned by the existing
storage maintenance/vacuum service.

Logical expiry is independent of physical cleanup. Monotonic timestamps and
byte prefixes permit indexed boundary seeks, and sequence arithmetic supplies
the count cutoff. The expired floor is inclusive: a cursor equal to the last
discarded sequence is valid, while one below it cannot acknowledge the gap.
Read-time checks see logical age expiry even if the GC worker has not run.

Stream floors combine already-collected routes with the indexed highest route
below the global logical cutoff. Losing history from one stream does not reset
an unrelated stream. Route deletion and its stream-floor update share one
transaction. Incarnation rebuilds additionally establish a baseline fence.

The existing storage maintenance worker performs sync cleanup outside the app
lock. A sync step changes at most 128 rows, including floor/counter updates,
route/journal deletion and expired deletion-revision cleanup. There are no sync
foreign-key cascades. Additional lease, device-version and directory-registry
cleanup joins this same owner in C3; it must preserve bounded physical work.

## Supported store identity boundaries

All replacement/restore operations below require a cold database: every old
Store, writer and ReadPool handle must be closed before replacing its files.
Concurrent normal opens remain unsupported, as in the existing store contract.

| Boundary | Detection and result |
|---|---|
| Create a new database/store | New independent random sync incarnation. Foreign cursors require a snapshot. |
| Ordinary restart/reopen of the intact file | Incarnation and key remain unchanged; a new process epoch fences old replies. |
| Atomic replacement of a closed SQLite file on Unix, including a copied backup containing its old meta | Persisted device/inode binding differs; opening rotates the sync incarnation. |
| Explicit in-place restore, including a backup copied over the same inode | Open through `Store::new_after_restore`, which rotates before readers are admitted. This is a store API boundary, not a new RPC or restore UI. |
| Incompatible sync definition at startup | Rotate the incarnation and establish a new baseline before reader admission. Missing sync storage objects fail closed and require an intact restore; opening never silently restarts the sequence. This is not a general database-repair tool. |

Known limits: an unannounced in-place copy/rollback preserving the file identity
cannot be distinguished from an intact restart. It must use the explicit restore
boundary. Unix inode reuse can also hide an unannounced replacement; filesystem
identity is a detector, not an external durable restore ledger. On non-Unix
platforms the fallback binds the canonical path and does not detect same-path
atomic replacement; use the explicit restore boundary. Live replacement,
independently copying WAL/database files, and external SQL bypassing supported
publication owners are outside this guarantee.

## Wire scopes

Wire 3.32 advertises `sync.projectTasks` and `sync.task`. Wire 3.33 adds
`sync.device` after the producer coverage below. The existing `sync.head`,
`sync.snapshot` and `sync.changes` POST query resources carry all three scopes;
the HTTP operation count remains 153. The SPA recognizes the device capability
without connecting a consumer; #997 owns that work.

## Device producer ownership (#995)

Device recovery uses materialized, durable summaries. The sole SQL writer
collects explicit touches and publishes final rows in the source transaction;
failed publication rolls the source back. Registry owners first persist a dirty
generation, then change memory/manifests/configuration, then reconcile an owned
authoritative snapshot. Deferred filesystem work retains that fence through its
last write-back. A failed marker prevents the effect; a failed reconciliation
leaves device reads retryably unavailable. Generation comparison clears only
the markers captured for that reconciliation.

| Record family | Publication owner | Semantic regression / negative control |
|---|---|---|
| `project`, `project_source` | App project/config mutators → `device_lifecycle` reconciliation | `lifecycle_project_sources_publish_registry_ids_and_only_durable_fields`; marker-failure refusal and restart-before/after-marker tests |
| `workspace`, `workspace_directory`, `badge` | Workspace lifecycle, adoption, reopen and capability refresh → the same fenced registry snapshot | `lifecycle_reconcile_tombstones_deleted_registry_members_without_runtime_badges`; deferred settlement, prepared delivery, failed reopen and background refresh tests |
| `agent`, `identity`, `inbox_conversation` | `write_agents`, attention, canonical item delta/reset and operation receipt hooks → `collect_device` | roster deletion, canonical aliases, summary/status/still-working, delivery/terminal attention, pending-only receipts, watch floor/muting, empty reset and hot historical-metadata tests |
| `inbox_task` | Existing task summary publication owner (including review, dependency and delivery metadata) | `task_capture_and_user_session_writers_publish_device_rows_and_deletions`; existing task publication/rollback and predicate fixtures |
| `inbox_capture` | Capture save/delete in `sync_transaction` | Same task/capture/session regression; capture body is excluded |
| `user_session` | Both session writers stage the actual committed meta value | Same task/capture/session regression; delayed-presence conditional-save tests |
| `archived_worktree` | Archive save → explicit family touch; startup backfill | `device_followup_archived_worktree_and_workspace_lifecycle_publish_durable_rows` |
| `workspace_lifecycle` | Canonical lifecycle meta save + bounded durable-field projection and absent-member tombstones | Same lifecycle-family regression; measured liveness/holds do not enter records |
| `owner_session`, `project_session` | Retain-agent and retain-owner transactions read maintained session points before deletion; project clear removes retained summary | `device_followup_retained_summaries_use_saved_facts_after_owner_history_is_removed`; short/long transcript SQL statement/VM cost comparison |

Registry project ids are resolved through a durable path-to-registry-id map.
Workspace links resolve owner root facts through the registry root map; a run's
owner id is not a workspace id. Missing mappings produce null links. Registry
mapping changes touch affected owners through indexes. Agent facts contain only
bounded directory fields and exact thread-generation identity, avoiding decoding
historical agent sessions/revisions on hot publication. Display text is clipped
before fingerprinting; opaque ids remain exact. Working state, availability,
active process/terminal counts and filesystem liveness remain outside records.

Unread attention is an incremental Fenwick counter over committed item deltas.
Summary/own-message/attention watermarks use indexed sparse item facts, excluding
progress/status/still-working output. Reset advances a generation in constant
writer work, with old-generation rows reclaimed by the bounded maintenance
worker. Retained session hooks read maintained points; they neither rescan nor
reuse the legacy timestamp-copy scans in the two canonical retention methods.
The legacy methods otherwise retain their existing behavior.

Startup backfill persists its phase and keyset after at most 128 source rows.
It completes compact facts before projecting aliases, and finishes all SQL-owned
families before lease admission. Interruption after a committed chunk resumes
from that cursor. Canonical transcript indexing is unchanged; sparse device
indexes begin empty and fill with those bounded chunks.

Device snapshots freeze a five-minute lease watermark. Admission is a short,
off-app-lock writer transaction that refuses an occupied writer immediately;
page reads use the recovery ReadPool. The quotas count only current-epoch,
current-incarnation leases (four/opening, sixteen/store), and admission prunes a
bounded invalid prefix. Every device read, including continuation helpers,
checks durable dirtiness. Snapshot pages seek `(first_seq, kind, key)` below the
frozen watermark, inspect at most 2,000 candidates plus one lookahead, and keep
the complete encoded reply within 1 MiB. A row payload over 64 KiB requests an
explicit reset. Lease expiry is checked again after SQL, assembly and encoding.
Device page continuations allow a bounded 64 KiB token for escaped opaque keys;
other continuation purposes retain the smaller bound.

Copy-on-write history accounts for record bytes, identity and version metadata,
including tombstones. Publication checks the 64 MiB retained-version limit
before retaining another version; pressure advances the durable lease fence
instead. Old leases then request recovery. Cleanup uses indexed seeks and the
shared 128-row maintenance budget, including generation garbage and accounting.
Tombstone scans retain a keyset cursor and revisit previously protected rows
when history becomes collectible.

These device hooks do not connect a SPA consumer. Capability recognition and
contract fixtures belong here; consumer synchronization is #997.
