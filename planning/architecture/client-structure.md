# Client structure and state

Keep the existing SPA and DOM renderer. Introduce dependency boundaries while migrating selected modules to TypeScript; no framework or state-management library is required. The same SPA serves browser and desktop page content. This is a target layout, not a bulk rename.

```text
spa/src/
  app/                     composition, route/shell lifecycle, dependency wiring
  platform/                browser storage, clocks, WebRTC/E2EE and device sessions
  protocol/                version adapters, wire types and runtime decoding
  data/
    cache/                 record addresses, IDB/volatile backend, transactions
    sync/                  snapshots, event reducers, cursor repair, subscriptions
    queries/               cached read models, query coverage and read scheduling
    commands/              typed RPC calls and operation/optimistic reconciliation
  features/
    tracker/               pure selectors, user actions, DOM views
    conversations/         thread/activity models, composer and views
    workspaces/            project/workspace models and interactive setup
    files/                 tree/editor/viewer/diff presentation
    agents/                picker, overview, surface and usage presentation
    inbox/                 cross-feature cached projections
    settings/              forms over cached preferences/capabilities
  terminal/                existing terminal stream/buffer/render boundary
  ui/                      shared DOM controls, motion, accessible components
  core/                    temporary compatibility facades while imports migrate
```

## Map existing modules, preserve behavior

| Current modules | Target |
| --- | --- |
| [bridgeApi](../../spa/src/core/bridgeApi/index.js), `sessionRpc`, `changeEvents` | `protocol` for wire adaptation; `platform` for transport/session; `data/sync` for subscription interest and ingestion. |
| [localCache](../../spa/src/core/localCache.js), `cacheScope`, `cacheLifetime` | `data/cache`, retaining canonical addresses and existing atomic read-mark merging. |
| [cacheSync](../../spa/src/core/cacheSync.js), `threadSync`, `trackerCache`, `conversationCache` | `data/sync` by entity family plus typed cache readers; extract narrow units from the existing pipeline. |
| [taskFeed](../../spa/src/core/taskFeed.js), `cachedRows`, `trackerIssueDetailsFeed`, `trackerAgentActivityFeed` | `data/queries` subscriptions/coverage; product joins into feature selectors. |
| [trackerDashboardModel](../../spa/src/core/trackerDashboardModel.js), `trackerAttentionModel`, `inbox`, `workspaceModel`, `agentRailModel` | Pure feature selectors; these already contain much of the desired client logic. |
| `createWork`, `threadSend`, `taskActions`, `adoption`, form sheets | Feature actions call `data/commands`; do not hold durable bridge workflow state. |
| [localUiState](../../spa/src/core/localUiState.js), router, focus/scroll/fold helpers | Local UI preference/state boundary; keep draft bodies separate from disposable UI journals. |
| `views`, `sheets`, existing render modules | Feature DOM views gradually; shared controls into `ui`. Preserve mounted shell, focus, scroll and element identity. |

## Import rules

- Views read cached queries/selectors and call feature actions. They never import raw sessions or render fetch/event bodies.
- Selectors are pure functions over typed records and explicit inputs such as current time; they cannot send commands, subscribe, read DOM or start timers.
- Actions express user intent through `data/commands`. Results are ingested into the cache. Temporary control state may show an in-flight click; the canonical entity still comes from cache.
- Sync owns network-to-record reconciliation and notification. The cache backend does not know tracker or agent business rules. Protocol adapters do not know DOM or navigation.
- Platform owns live handles, cryptographic material and connection mechanics. Rendered connection/diagnostic facts go through the data layer; secret keys and sockets do not become ordinary cache records.
- The composition root wires interfaces. Feature models cannot depend on `App` globals; inject read/command/clock interfaces at controller boundaries. Preserve compatibility facades until imports and tests move.

## Four kinds of state

| State | Owner and lifetime |
| --- | --- |
| Canonical work state | Bridge records/services: issues, messages, watches, read marks, agents, accepted operations. Client has a replaceable replica. |
| Cached entities and query coverage | Client data layer: committed snapshots, revisions, tombstones, provisional overlays, cached activity and freshness. Mount reads first; updates notify only after commit. |
| User drafts and preferences | Explicit client persistence for unsent content; shared preferences through bridge settings commands. Durable drafts must not be cleared by replica-cache schema upgrades or lifetime eviction. |
| Ephemeral UI/runtime state | Focus, selection, animation and open panels; local preferences may persist via `localUiState`. Socket/timer objects stay platform/controller-local and are not canonical work state. |

Use one authoritative record source per entity; read models may memoize by revision but must not become another writable entity store. A typed cache key maps to a typed record and explicit missing/stale/partial/complete result. A failed IDB write cannot trigger a view to reread nonexistent data; provide the same cache API over bounded volatile storage or expose a failure state.

Draft preservation above is a target, not today's guarantee. [localUiState.js](../../spa/src/core/localUiState.js) currently writes `ui-*` drafts through the same [localCache](../../spa/src/core/localCache.js) records store that schema upgrades delete. Its sessionStorage exit journal covers unfinished writes, not general draft migration. Before any cache schema bump, split or migrate these records into separately retained persistence, with upgrade/restart and eviction tests. Preserve unsent drafts even when disposable replicas must be rebuilt.

The latest [cachedBodies](../../spa/src/core/cachedBodies.js) has a mount-local path for oversized/noncacheable bodies pending #95 pagination work. Its per-mount map has no generic total-memory bound; add an explicit byte/entry cap as part of that work. Preserve the existing explicit exception; do not extend it to migrated entity logic or use it to bypass #82. Terminal bytes similarly use the terminal's stream/buffer/reset contract, while terminal lists and status records are cached entities.

## First vertical slice and acceptance

Start with the tracker: protocol decoder -> typed issue/cache record -> existing dashboard selector -> existing view. Keep runtime schemas separate from TypeScript types: JSON, IDB data and old bridge versions remain untrusted shapes. Then migrate conversation identity/updates and the workspace feed. Add package/import checks after the first boundary stabilizes, not a giant directory move before behavior is understood.

Retain Vitest and jsdom, the fake-IDB setup, and real browser layout tests. For each slice check a late/absent payload, real cache notification wiring, partial query coverage, stale responses, cross-tab merging and optimistic reconciliation. Check focus/scroll persistence as modules move. `tsc --noEmit` is an additional gate, not a replacement for these runtime tests. The detailed ownership moves are in [projections](migrations/01-client-projections.md), [contracts/cache](migrations/02-contracts-and-cache.md) and [commands](migrations/03-client-commands.md).
