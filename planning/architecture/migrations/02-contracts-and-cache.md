# Contracts, factual events and client cache

**Goal:** independently deployable clients can derive their views from a reliable replica of bridge-owned facts. Always-on services remain authoritative. This plan adds capabilities where needed; it does not replace the E2EE transport or prescribe an event-sourced database.

## Starting point

The baseline reports API **1.13.0** in [api/mod.rs](../../../bridge/src/api/mod.rs). Typed families live in [api/v1](../../../bridge/src/api/v1/mod.rs); the SPA selects [versioned adapters](../../../spa/src/core/bridgeApi/index.js). [changes.rs](../../../bridge/src/changes.rs) already carries bounded bodies, invalidations and thread cursors. Its general subscription stream is not a durable replay log. [cacheSync.js](../../../spa/src/core/cacheSync.js) remains the ingestion coordinator.

Since the original audit, [localCache.js](../../../spa/src/core/localCache.js) gained `mergeCachedAtomically`, used by [trackerCache.js](../../../spa/src/core/trackerCache.js) to keep read-through marks monotonic. Preserve this work. It does not by itself establish general per-entity revision ordering or a lossless snapshot/subscribe handshake.

## Contract work map

| Current location / concern | Target and owner | Reason / always-on requirement | Dependencies and order |
| --- | --- | --- | --- |
| `api/v1`, `app/rpc`, `app/mcp`: adapter calls reach `AppState` | Typed service facade in `services/contracts.rs`; thin API/MCP mapping | Commands reach one authority from every client; workflow decisions stay services. | P0 contract inventory; P3 extraction without wire changes first. |
| `app/facts`, `app/board/views`, tracker inbox: product-shaped snapshots | Versioned entity/query DTOs from service query facades | Client projections need factual IDs, ownership, timestamps, cursors and completeness. Bridge must query data unavailable in a bounded client cache. | P2 additive fields/queries before retiring old shapes. |
| `changes`: coalesced state and subscription delivery | Publisher consumes committed facts and bounded query snapshots | Publication must run browser-closed; publisher neither changes an issue nor chooses reminder recipients. | P2 snapshot coverage; P3 durable intent/outbox only where missed side effects require it. |
| `store/operations`, `thread.operation`: durable post receipts | Explicit accepted-operation contract extended to chosen mutating use cases | Local browser dedup cannot arbitrate multiple clients or uncertain provider delivery. | P2 operation ID/payload binding, preconditions and reconciliation; preserve existing delivery states. |
| `cacheSync`, `threadSync`, `conversationCache`, `trackerCache` | One normalized, revision-aware ingestion pipeline | Client replication is client-local. It does not execute bridge workflows. | P1 consolidate existing paths; P2 per-entity revisions and transaction batches. |
| `localCache`: IDB failures stand down | Same cache interface with bounded volatile backend, or explicit unavailable state | #82 must not degrade to rendering incoming payloads. Volatile cache is not durable storage. | P0 agree supported fallback; P1 backend implementation/tests. |
| `issues.list` lacks limit/cursor at baseline | Bounded query pages with opaque cursor and coverage | Query filtering/order is device data access; dashboard grouping remains client. | P2, #85; coordinate existing client-side pagination/#84. |

## RPC and event vocabulary

Keep project/source/workspace/checkout, plural `issues.*`, conversation/thread, agent/session, git/files, terminal and settings families. Mutations express an explicit requested action; multi-step accepted work returns an operation receipt. Keep service-side permissions, final preflight, stored preferences and headless defaults. No generic SQL, arbitrary shell or client-uploaded policy interpreter is introduced.

Extend `session.hello` with explicit support for new semantics, not merely field names. Define scoped factual event families for entity upsert/deletion, conversation append/update, activity/surface observation, git/files changes, usage limits and operation progress. Terminal output/reset remains a separately sequenced stream. Human labels, dashboard groups and inbox position are not canonical entities.

Proposed additions, subject to wire review:

- Stable entity/conversation identity; per-entity revision and tombstone; session generation for runtime observations.
- Event stream incarnation plus an opaque replay/coverage cursor. A global cursor may skip filtered events; do not infer data loss from integer gaps alone.
- Snapshot/query response carrying the matching coverage cursor and completeness, not just an array.
- Command operation ID, payload binding and expected revision where races matter; ACK identifies accepted revision/status. Events may precede ACKs.
- Either complete records or explicitly defined patches with a base revision. Missing field, deletion and unavailable observation are different states.

Use an atomic snapshot+cursor followed by subscribe-since, or subscribe-first with buffered events and a snapshot fence. The current ordered read-then-subscribe path must not be assumed lossless. If replay retention expires, say reset-required and reconcile snapshots while preserving stale paint. If a durable replay log is deferred, ship an explicit bounded snapshot reconciliation contract instead of claiming replay support.

## Cache rules (#82)

`RPC/event -> adapter validation -> sync reducer -> cache transaction -> address notification -> cache reread -> selector -> view`

Cache addresses include device, canonical entity, record kind and sub-key. Query records additionally bind normalized filters, order, limit, cursor and snapshot identity; they reference canonical entities. A page missing an entity is not a tombstone. A missing record is not an empty result. Persist freshness, coverage and source revision explicitly.

Reject older revisions; merge message updates using their update sequence; keep operation IDs for provisional reconciliation. Commit records and consumed cursor together before announcing. Related records whose intermediate state would be misleading need a new multi-address atomic batch API covering entity records, query coverage and consumed cursors. Existing `mergeCachedAtomically` protects one address only; preserve that primitive and test the new batch for rollback and absence of intermediate notifications. Neither the module-local `mergeCached` queue nor a browser lock coordinates all writers by itself.

Invalidate or update affected page membership when a filter-changing issue event arrives. Stable number-descending #85 pagination must bind the cursor to project/filter/order and define snapshot or live-traversal semantics. Global dashboard counts/grouping require complete coverage or supported server queries; they cannot be inferred from the first page. Keep bounded timeline/count queries to avoid per-row full-history downloads.

Reconnect, return from suspension and bridge incarnation change reconcile authoritative records. Optimistic changes remain provisional until confirmed. Unsent drafts and accepted operation records need different retention from disposable read replicas.

## Compatibility and checks

Additive methods/fields use a minor version and capability gate. Preserve old view-shaped methods for old clients. A new client normalizes supported old replies into its cache and degrades only unsupported features. The current facade [drops unknown parameters](../../../bridge/src/api/v1/mod.rs#L197); never depend on an old bridge honoring a new revision or cleanup parameter. Use a newly negotiated method/semantic version for safety-relevant additions.

Keep wire version separate from local cache schema version. General cache eviction must not remove draft/intent durability. The current `ui-*` draft records share the replica store, whose schema upgrade deletes that store: migrating drafts into separately retained storage is a prerequisite to any such upgrade, not an existing guarantee. See the [client state plan](../client-structure.md). Breaking field semantics or removal requires a major/versioned contract; rollout order is supporting bridge, gated app use, then eventual compatibility retirement. #1/#2/#3 are related wire-discipline issues; #30/#74 cover uncertainty/catch-up.

Test old/new combinations, unknown kinds/fields, duplicate/reordered events, stale pull after fresh push, tombstones, two tabs, two devices, cursor expiry, reconnect handoff and crash after receipt commit. Preserve scoped bounds, E2EE and off-lock I/O. Bridge suite/contract fixtures and SPA lint, runtime tests and type-check (once added) are independent gates. This plan does not require a full protocol rewrite before P1 presentation moves.
