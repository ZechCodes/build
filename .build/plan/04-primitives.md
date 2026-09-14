# Primitives inventory for the multi-device stages

Status: plan (2026-09-13). Written after reading `00`–`03` and every source file
they cite, at the commit `f367a124`. Every `file:line` below was opened and
checked; lines are `spa/src/…` unless another root is named. Builders work from
this doc plus the stage doc for their stage; where the two disagree, this doc
has already amended the stage doc (section 5).

Note on verification: no container runtime is usable in this environment, so
the two-bridge browser pass the stage docs describe cannot be run. The jsdom
suites are the verification for every stage. Say so in each stage report.

## 1. Existing primitives to reuse

Each entry: what it does today, which stage reuses it, and whether its
interface must widen.

### Sessions and connection

- **`openRelaySession`** `core/session.js:48-151`. Opens one E2EE relay session
  and returns `{ deviceId, call, onPush, peer, onCarrier, close }`. Takes
  `preferDeviceId`, `waitForDevice`, `isPaused`, `onDeviceKey`,
  `onDeviceOffline`, `onLost`, `onPush`. `relayLink.js:144-148` waits only for
  the `device_key` of `preferDeviceId` when one is given (forever with
  `waitForDevice: true`, `deviceWaitMs` otherwise), so a session opened with a
  device id lands on that device or throws. Reused by every stage, unchanged.
  Every per-device callback (`isPaused`, `onLost`, `onPush`) closes over the
  device id passed as `preferDeviceId`.
- **`openAppSession`** `connection.js:41-62`. Wraps `openRelaySession` with the
  app's globals: `isPaused: () => App.offline`, `onLost: goOffline`,
  `onPush → dispatchChangeEvent(payload)`. Stage 1 widens it into
  `openDeviceSession(deviceId, { waitForDevice })` (section 2); the three
  callbacks become per-context.
- **`openDeviceSettingsSession`** `connection.js:66-82`. The proof that N
  concurrent sessions work: opens a second relay session pinned to one device
  and refuses a mismatch. Reused as the pattern for `openDeviceSession`;
  `views/deviceSettings.js:6,100` keeps using it as is (its page owns its own
  transport and must keep doing so).
- **`adoptSession`** `connection.js:162-183`. Stores the session on `App`,
  calls `adoptApplicationScope`, resets the feed on a device change, repaints
  the picker, flushes captures, starts the peer upgrade. Stage 1 replaces its
  body with `adoptDeviceSession(session)` from the registry plus the
  home-only side effects (`flushCaptures`, alias re-pointing).
- **`upgradeToPeer` / `adoptPeerLink` / `dropPeerLink`** `connection.js:97-144`.
  Module-level `peerLink` (`:86`); `adoptPeerLink` bails when
  `App.session !== session` (`:118`) and hands the term channel to
  `terminalsRideOn(link.term)` (`:131`). Stage 1: `peerLink` moves onto the
  context, the guard becomes `contextFor(session.deviceId)?.session !== session`,
  and only the context the terminal socket is attached to may call
  `terminalsRideOn` (hazard, section 4).
- **`greetLiveBridge`** `connection.js:150-160`. Calls `greetBridge` with
  `isCurrent: () => App.session === session && App.chatRepository === repository`
  and `onGreeting → repository.configureCapabilities`. Stage 1 widens to
  `greetLiveBridge(context)`; `session.onCarrier(() => greetLiveBridge(context))`.
- **`goOffline` / `resume`** `connection.js:196-252`. Global: `App.offline`,
  `App.offlineSince`, one `reconnectTimer`/`reconnectDelay`, `App._resuming`.
  Stage 1 makes both take a `deviceId` and keep their timer and flag on the
  context.
- **`switchDevice`** `connection.js:255-285`. Opens the new session first,
  closes the old, remembers the choice, retargets terminals, renders. Stage 1
  renames it `setHomeDevice(deviceId)` and deletes the close; stage 3 deletes
  the picker's call to it (`devices.js:102`).
- **`offlineBannerText`** `core/text.js:16-19`. One device's banner sentence.
  Stage 1 keeps it for the one-offline-device case and adds
  `allDevicesOfflineText()` beside it.

### Devices

- **`refreshDevices` / `deviceName` / `pinnedDeviceTransportKey` /
  `markDeviceOnline` / `markDeviceOffline`** `devices.js:12-55`. `App.devices`
  is the api's list, statuses patched live by the relay pushes, whichever
  session heard them (`relayLink.js:123-127`). Stage 1 adds one line to
  `markDeviceOnline`: `openDeviceSessions()` (a device joining after boot).
  `App.devices` order is the order every merge and every device menu uses.
- **`paintDevicePicker`** `devices.js:57-73`. Labels the toggle with
  `App.session?.deviceId || App.selectedDeviceId` (`:62`), title "Which device
  runs your tasks". Untouched in stages 1–2; stage 3 relabels it as the filter
  ("All devices" row first, title "Which devices the inbox shows") and
  `selectDevice` (`:97-110`) writes `App.deviceFilter` instead of calling
  `switchDevice`. `pickerKeydown`/`pickerFocusIndex` (`:125-145`) are reused
  as they are.
- **`onlineStickyDeviceId`** `core/devicePolicy.js:8-11`. Sticky device if
  online, else null. Unchanged; `homeDeviceId` (section 2) is written next to
  it and calls it.
- **`rememberSelectedDevice`** `app.js:110-114`. Persists
  `build.selectedDeviceId`. Stage 3's Creation device select writes through it.

### Scope, repository, catalog

- **`adoptCacheScope` / `clearCacheScope` / `currentCacheScope` /
  `setCacheDevice` / `cacheDeviceId`** `core/cacheScope.js:32-57`. One module
  singleton; adopting another device disposes the first (`:35`), which
  `cacheScope.test.js:22-35` pins. Stage 1 turns the module into a registry
  (`scopeFor`/`releaseScope`, section 2) and keeps `adoptCacheScope` as the
  home alias that no longer disposes the previous scope; stage 3 deletes the
  three compatibility exports once their readers (`console.js:155`,
  `changesReview.js:97`, `gitPane.js:389`, `files.js:151`,
  `worktreeReview.js:77,112`, `taskReview.js:88,101`, `agentRail.js:180`)
  take the scope from what their view hands them (stage 2).
- **`createCacheScope`** `core/cacheScope.js:5-26`. `{ deviceId, key,
  active(), address(parts), dispose() }`; `address` stamps `deviceId`. The
  per-device registry stores these unchanged.
- **`adoptApplicationScope` / `disposeApplicationScope`** `app.js:53-83`.
  Retarget-or-recreate the repository, recreate `App.viewingContext`, null the
  catalog. Stage 1 routes it through `adoptDeviceSession` for the home device
  so `appScope.test.js` stays green; stage 3 deletes both and folds the four
  cases of `appScope.test.js:14-85` into `deviceContexts.test.js`.
- **`createChatRepository`** `core/chatRepository.js:493`, with
  `configureCapabilities` (`:551`), `retarget(nextCall)` (`:668`), `dispose()`
  (`:674`); `scopeKeyOf` (`:23-29`) keys drafts by `scope.key`. One instance per
  context; interface unchanged.
- **`loadModelCatalog` / `refreshModelCatalog`** `app.js:88-108`. Per bridge
  (`models.list`), guarded by "still the same scope" (`:100`). Stage 3 moves
  them onto the context as `context.modelCatalog()` /
  `context.refreshModelCatalog()`; readers today: `composeView.js:178,321`,
  `createWork.js:42`, `settings.js:127-131`.
- **`App.viewingContext`** `app.js:27`, created once and recreated on a device
  switch (`app.js:60`). It is the reader's position, not a device's: with N
  contexts it stays one object on `App`, created once, and every repository
  is given the same one. `adoptDeviceSession` must not recreate it.

### Feed and cache

- **`liveFeedSnapshot`** `core/taskFeed.js:53-73`. The one place the wire is
  read; bridges `project_id → id`. Stage 1 widens it to
  `liveFeedSnapshot(board, projectList, deviceId)` and exports it (pure,
  tested alone).
- **`tick` / `ownsFeedContext` / `seedFromCache`** `core/taskFeed.js:50-111`.
  Read `App.call`/`App.session`/`App.cacheScope`. Stage 1: `tick(context)`
  guarded by `context.active()`; `seedFromCache` reads every device in
  `App.devices` (today `App.selectedDeviceId`, `:103`).
- **`subscribeFeed` / `startFeed` / `stopFeed` / `refreshFeed`**
  `core/taskFeed.js:28-138`. Shapes are unchanged for consumers; the delivered
  snapshot grows `devices`. `resetFeedScope` (`:36-39`) is deleted in stage 1
  (its only caller is `connection.js:174`).
- **`primaryRunIdFor`** `core/taskFeed.js:45-48`. Matches `e.project_id ===
  projectId`; ambiguous under a merge. No `src` caller (`taskFeed.js:45` is
  the only definition); 14 test files stub it (`agentRailDom`,
  `agentRailIsolation`, `agentSurfacesOverlay`, `cacheConsole`,
  `captureDecisionDom`, `composeDom`, `composerFocus`, `createWork`,
  `inboxDom`, `inboxProjectsDom`, `railComposerPinned`, `shellSkeleton`,
  `threadIdentity`, `toolbarDom`), so a rename or deletion touches every one
  of those mocks.
  Stage 1 widens it to `primaryRunIdFor(feed, projectKey)` or deletes it.
- **`watchChanges` / `dispatchChangeEvent` / `armChangeEvents` /
  `changeEventsArmed` / `pollIntervalMs` / `refetchEverything` /
  `greetBridge`** `core/changeEvents.js:44-214`. One `armed` flag (`:39`), one
  watcher set (`:40`). Stage 1 widens every one of them by a `deviceId`
  (section 2); `greetBridge` gains a `deviceId` option (the stage doc said
  "unchanged in shape" — amended). Callers of `watchChanges` today:
  `taskFeed.js:121`, `cacheSync.js:267`, `gitPane.js:1558`, `issueView.js:833`,
  `changesReview.js:536`, `agentRail.js:2035`, `captureDecision.js:31`,
  `views/archive.js:83`, `branchView.js:558`.
- **`syncContext` / `refreshEntity` / `onSnapshot`** `core/cacheSync.js:39-270`.
  `activeRows`/`entityWatchers` keyed by bare entity id (`:35-37`); the feed
  record is written under `{ deviceId, entityId: "", kind: "feed" }` (`:243`).
  Stage 1: `syncContext(context)`, keys `${deviceId}|${entityId}`; `onSnapshot`
  is ratcheted at 14 (`:238`) so the per-device loop goes into a new
  `syncDeviceSnapshot(deviceId, view)` and `onSnapshot` only iterates
  `snapshot.devices`.
- **`recordKey` / `evictEntity` / `cachedEntityIds` / `cachedSubKeys`**
  `core/localCache.js:91-160`. Keys are `deviceId|entityId|kind|sub`; every
  reader already takes `deviceId`. Unchanged; this is why per-device caching
  costs nothing.
- **`rpcReadKey` / `coordinatedRead`** `core/readRequests.js:31-40,110-130`.
  Already keyed by `deviceId` + `requestScope` identity. Unchanged; stage 2
  makes every caller (`cacheSync.js`, `fileDiffs.js`, `taskReview.js`,
  `worktreeReview.js` — two sites each) pass the route context's id and scope.

### Inbox, projects rail, toolbar

- **`entryRoute`** `core/inbox.js:172-188`. Pure; complexity 9. It returns
  three route objects: capture (`:182`), issue (`:185`) and branch (`:187`).
  Stage 2 adds `deviceId: item.deviceId` to the issue and branch routes (no
  new branch); the capture route stays as it is — `#/capture/<id>` is not
  per-device (`00-multi-device-design.md:61`, under Decisions). The
  stage 1 read-only rule does NOT go here (amended; section 5).
- **`entryKeyOf`** `core/inbox.js:199-202`. `capture:<id>`, else the entity
  id, else `issue:<project_id>` / `branch:<project_id>:<branch>`. Stage 1
  substitutes `projectKey` for `project_id` in the last two (same shape).
- **`dismissParamsOf`** `core/inbox.js:211-216`. Wire params stay bare
  (`project_id`), unchanged.
- **`inboxEntries` / `cacheableEntityIds` / `activeEntryKey`**
  `core/inbox.js:402-453` (`activeEntryKey` is `:437-453`). `activeEntryKey`
  matches a branch route by `projectId + branch` (`:447`); stage 2 adds
  `deviceId` to that match (one `&&`, the function is not ratcheted: eslint
  `complexity` reports 7 today).
- **`inboxRowHtml` / `quietRowHtml` / `captureRowHtml`** `core/inbox.js:513,
  550, 646`. Give `inbox-unroutable` to a row with `route: null` (`:527`,
  `:559`, `:661`). The tooltip is `rowTooltip(entry)` (`:504`). Stage 1's
  "Opens once this page can name its device" title rides `entry.title`-style
  data set in the wiring, not a new branch in these ratcheted functions.
- **`projectsNamed` / `projectBlocks` / `blockIsFolded` / `projectHeadHtml` /
  `projectBlockHtml`** `core/inboxProjects.js:35-154`. Blocks keyed
  `project:<id>` (`:84`), folds by `block.id` (`:110,120,145`), DOM attributes
  `data-project`, `data-project-fold/open/create` carry `block.id`
  (`:126-130,150`). Stage 1 substitutes `projectKey` for `id` in every one of
  those (amended; section 5). `projectHeadHtml` is ratcheted at 12.
- **`loadProjectFolds` / `persistProjectFolds`** `core/railMode.js:45-64`,
  key `build.inbox.folded` (`:12`). Map keyed by whatever string the wiring
  passes; stage 1 passes `projectKey` (old bare-id entries read as "nothing
  said", which is acceptable).
- **`inboxView`** `core/inboxView.js`. `App.call` at `:102,104,540,578,607,609,
  622,623`; `folds`/`blocksPainted` (`:73,76`); `rowUi.activeProjectId` from
  `App.route.projectId` (`:204`); `toggleFold` (`:437-443`) / `expandFold`
  (`:446-452`).
  Stage 1 rewrites the eight call sites to `verbTarget(entry)` (section 2).
- **`projectRoute`** `core/projectModel.js:3-12`. Stage 2 adds
  `deviceId: project.deviceId`.
- **`projectMenuModel` / `workMenuModel` / `toolbarIdentity`**
  `core/toolbarModel.js:62-118`. Match by `project.id` / `item.project_id`.
  Stage 2 matches by `projectKey` and `deviceId` and returns routes with
  `deviceId`. `toolbar.js:40` `SCOPE_KEY = "build.toolbar.project"` stores a
  bare id (`:95,422`) — stage 2 stores a `projectKey`.
- **`captureDecisionView`** `core/captureDecisionView.js:57-58` reads
  `next.projects`; stage 1 reads `next.devices[homeId]`. `ensureProject`
  (`:70`, ratchet 12) is not touched.
- **`goFromInbox`** `core/inboxShell.js:89-97`. Passes the route through;
  unchanged once routes carry `deviceId`.

### Router and resolve

- **`routeFromHash` / `surfaceFromHashPath` / `hashFromRoute`**
  `core/router.js:134-224`. Ratchets at `:141` (28) and `:209` (12). Stage 2
  peels/writes the device segment in new helpers (section 2) and moves the
  existing switch body to `surfaceFromSegments(parts)` under the same ratchet
  line. `#/device/<d>` → `{ name: "device", id }` (`:150`) stays.
- **`resolveLegacyRoute` / `inNamedProjectFirst` / `branchRouteFor`**
  `core/routeResolve.js:8-49`. Pure over `items`. Stage 2: candidates are
  filtered then handed to `pickDevice`; routes gain `deviceId`; a
  `kind: "project"` ref also needs `projects` (amended).
- **`renderResolving`** `views/resolving.js:13-29`. Waits one snapshot, calls
  `go(resolveLegacyRoute(App.route, feed.items) || inbox)`. Stage 2 passes the
  merged feed and the device policy inputs.
- **`go` / `applyRoute` / `markRoute` / `render`** `app.js:135-251`. Stage 2:
  `applyRoute` and `markRoute` normalise through `withDeviceOrResolve`; `render`
  calls `retargetTerminals()` when `route.deviceId` changed. `VIEWS` (`:218`)
  is the route-kind table already.

### Views and terminals

- **`branchView`** `views/branchView.js`: `branchScope` (`:66`), `callRpc =
  App.call` (`:220`), `createAdopters(callRpc)` (`:225`), `isOffline: () =>
  App.offline` (`:342,355`), `project.init_git` (`:402`), and a hand-built
  `markRoute({ name: "branch", projectId, branch, tab: "files", file })`
  (`:391`, the `onFileOpen` option of the `renderFilesTab` bag that starts at
  `:384`) that must carry `deviceId` in stage 2. It already passes
  `callRpc`/`scope` down to `renderFilesTab`, `mountGitPane`,
  `createTaskReview`, `createWorktreeReview` — stage 2 adds `cacheScope` and
  `chatRepository` to those option bags.
- **`issueView`** `views/issueView.js:24` `callRpc = App.call`; `syncHash`
  (`:33-41`) rebuilds `App.route` by hand and must keep `deviceId`.
- **`agentRail.railChatDependencies`** `core/agentRail.js:179-192`. Already
  takes `context.cacheScope` / `context.chatRepository` / `context.viewingContext`
  with `App.*` fallbacks; stage 2 deletes the fallbacks (each `||` is a
  complexity point; removing them lowers the score).
- **`mountConsole(host, context)`** `core/console.js:153`; `currentCacheScope()`
  (`:155`); `App.call("branch.get")` (`:194`). Stage 2: the context gains
  `deviceId`, and the caller becomes `routeContext(App.route).call`.
- **`terminalManager` / `terminalsRideOn` / `retargetTerminals`**
  `terminal/manager.js:34-85`. `preferDeviceId` (`:45`) and the wanted id
  (`:83`) read `App.session?.deviceId || App.selectedDeviceId`. Stage 2: a
  single `terminalDeviceId()` = `App.route.deviceId || homeDeviceId(...)`
  used by both.
- **`gate.enterApp` / `watchForOnline` / `renderWaiting` / `boot`**
  `views/gate.js:32-224`. Stage 1: `enterApp` awaits the first success of
  `openDeviceSessions()`; stage 3: `renderWaiting` is what the app shows when
  `liveContexts()` empties after boot.
- **`composeView.canSend` / `flushCaptures`** `core/composeView.js:50,120-127`.
  Stage 3 reads the home context.
- **`views/settings.js`** `:100,111,203` (`project.list`, `settings.get/set`)
  — stage 3 moves these panels to `views/deviceSettings.js`, which already owns
  its transport (`deviceSettings.js:44-49`).
- **`views/archive.js:55`** `App.call("archived.list")` — not named by any
  stage doc; stage 3 owns it (amended). The rows it fetches go through
  `archiveRows` → `toRow` in `core/archive.js:66-67` (ratcheted at 12); the
  view file itself has no ratchet.

## 2. Primitives to create

Signatures are the contract; bodies are the builder's. "Stage" is where it is
created; later stages may widen but not rename.

### 2.1 `core/deviceKey.js` — stage 1

```js
export const deviceKey = (deviceId, projectId) => `${deviceId}/${projectId}`;
/** → { deviceId, projectId } split on the FIRST "/", or null when `key` is not
 *  a string or either half is empty. Bridge ids are `proj-<n>` and api device
 *  ids are uuids, so neither half contains "/". */
export function splitDeviceKey(key);
```
Replaces: nothing (nothing concatenates today). Test: `test/deviceKey.test.js`.

### 2.2 `core/deviceContexts.js` — stage 1 (widened in 2 and 3)

The context object (one per paired device that has ever had a session):
```js
{
  deviceId,            // api uuid
  session,             // { deviceId, call, onPush, peer, onCarrier, close } or null while offline
  call,                // (method, params, timeoutMs) => Promise — session.call, retargeted on re-adopt
  cacheScope,          // scopeFor(deviceId); live until retire
  chatRepository,      // createChatRepository({ scope: cacheScope, call, viewingContext: App.viewingContext })
  offline,             // boolean — one writer: connection.js goOffline/resume via setContextOffline
  offlineSince,        // ms or null
  peerLink,            // openPeerLink result or null
  reconnect: { timer, delay, resuming },   // owned by connection.js
  active(),            // registry still holds this object && cacheScope.active()
  modelCatalog(),      // stage 3: cached models.list, guarded by active()
  refreshModelCatalog()// stage 3
}
```
Registry:
```js
export function contextFor(deviceId)          // context or null
export function liveContexts()                // registered, session !== null, !offline; App.devices order, unknown ids last
export function knownContexts()               // every registered context (offline included), same order — the rail's grey rows
export function adoptDeviceSession(session)   // create or retarget the context for session.deviceId; returns it
export function retireDeviceContext(deviceId) // dispose repository, releaseScope, close session, drop feed entry, disarm events
export function setContextOffline(deviceId, { offline, sinceMs })
export function routeContext(route)           // stage 2: contextFor(route.deviceId) or null
export function homeContext()                 // stage 2: contextFor(homeDeviceId(App.devices, App.selectedDeviceId))
export function resetDeviceContexts()         // tests
```
Rules: `App.devices` is read lazily inside `liveContexts()` (the module is in an
import cycle with `app.js`). `adoptDeviceSession` on a known device keeps
`cacheScope` and `chatRepository`, calls `chatRepository.retarget(session.call)`,
swaps `call`, clears `offline`. Replaces `adoptApplicationScope` (stage 3
deletes it; stages 1–2 route it through here for the home device).
Tests: `test/deviceContexts.test.js` (stage 1: adopt two, re-adopt one, retire
one; stage 3: absorbs the four cases of `appScope.test.js`).

Aliases (stages 1–2 only, `app.js`): `pointAliasesAt(context)` copies
`session/call/cacheScope/chatRepository/offline/offlineSince` onto `App` as
plain fields — plain, because `inboxDom.test.js:144` and `taskFeed.test.js`
assign `App.call` directly. Stage 3 deletes the six fields and the function.

### 2.3 `core/cacheScope.js` registry — stage 1

```js
export function scopeFor(deviceId)       // create on first ask; same object until released
export function releaseScope(deviceId)   // dispose and forget
export function adoptCacheScope(deviceId)// = scopeFor(deviceId) and points the home alias; never disposes another device's scope
export function clearCacheScope()        // release every scope (tests, sign-out)
// currentCacheScope / cacheDeviceId / setCacheDevice: home aliases, deleted in stage 3
```
Replaces the singleton `currentScope` (`cacheScope.js:28`). Test:
`cacheScope.test.js` — its second case becomes "adopting another device
leaves the first scope live; releasing it retires it".

### 2.4 Merged feed — stage 1, `core/taskFeed.js` (+ pure `core/feedMerge.js`)

```js
// per-device snapshot (what liveFeedSnapshot returns):
{ items, plans, runs, externalWorktrees, pending, primaryChanges, projects, cached? }
//   every row: + deviceId, + projectKey when it names project_id
//   every project: + deviceId, + projectKey
export function liveFeedSnapshot(board, projectList, deviceId)     // pure, exported
export function mergeFeeds(byDevice /* Map<deviceId, snapshot> */, deviceOrder /* string[] */)
//   → { ...the seven arrays concatenated in deviceOrder (ids not in the order last),
//       devices: { [deviceId]: snapshot }, cached: every entry cached }
export function subscribeFeed(fn)          // unchanged; delivers the merge
export function startFeed(intervalMs)      // one tick + one board watcher per liveContexts() entry
export function stopFeed()
export function refreshFeed(deviceId = null) // all live contexts, or one
export function dropFeedDevice(deviceId)   // called by retireDeviceContext: delete entry, deliver merge
export function primaryRunIdFor(feed, projectKey)
```
`tick(context)` writes `byDevice.set(context.deviceId, …)` only if
`context.active()`. Replaces `resetFeedScope`, `ownsFeedContext`, module `last`.
Tests: `taskFeed.test.js` (two contexts → merged in order; late answer from a
retired context dropped; `devices[id]`), `feedMerge.test.js` for the pure
merge.

### 2.5 Device-tagged change events — stage 1, `core/changeEvents.js`

```js
export function armChangeEvents(greeting, deviceId)   // per-device armed map; re-times that device's and the any-device watchers
export function disarmChangeEvents(deviceId)          // retire
export function changeEventsArmed(deviceId)           // that device; with no id: every known device armed (and at least one known)
export function pollIntervalMs(fastMs, deviceId = null)
export function watchChanges({ refresh, intervalMs, entity = null, deviceId = null, catchUpOnVisible, pausesWhileHidden })
//   watcher gains `deviceId` and a predicate `hears(d)`: deviceId === null ? () => true : (d) => d === deviceId
export function dispatchChangeEvent(payload, deviceId)
//   unarmed for deviceId → false; board.changed → boardScoped watchers whose hears(deviceId);
//   entity.changed → watchers naming payload.id (any device; ids are uuids)
export function refetchEverything(deviceId = null)    // that device's watchers + any-device ones; no id: all
export function resetChangeEvents()
export async function greetBridge(call, { deviceId, isCurrent, onGreeting })  // arms and refetches for deviceId
```
Test: `changeEvents.test.js` (B's `board.changed` refreshes B's feed watcher
and the merged inbox watcher, not A's; armed per device; any-device cadence
stands down only when every device is armed).

### 2.6 Per-device offline — stage 1, `connection.js`

```js
export function openDeviceSession(deviceId, { waitForDevice = false } = {})
//   openRelaySession with preferDeviceId: deviceId, isPaused: () => contextFor(deviceId)?.offline === true,
//   onLost: () => goOffline(deviceId), onPush: (p) => !isSignaling(p.type) && dispatchChangeEvent(p, deviceId)
export function openDeviceSessions()
//   for every App.devices entry with status "online" and no live context, concurrently;
//   → { first: Promise<context>, settled: Promise<context[]> } — the gate awaits `first`
export function goOffline(deviceId)      // setContextOffline, dropPeerLink(context), close session, paintOfflineBanner, resume(deviceId)
export function resume(deviceId)         // openDeviceSession(deviceId, { waitForDevice: true }); backoff on context.reconnect
export function greetLiveBridge(context)
export function setHomeDevice(deviceId)  // remembers, opens if needed, pointAliasesAt (stages 1–2), retargetTerminals, render; closes nothing
export function paintOfflineBanner()     // hidden when liveContexts().length > 0 unless exactly one known context is offline → offlineBannerText(name, since); none live → allDevicesOfflineText()
// core/text.js: export const allDevicesOfflineText = () => "All devices are offline — tasks will resume when one reconnects.";
```
Replaces `openAppSession`, global `goOffline`/`resume`, `switchDevice`,
module `peerLink`/`reconnectTimer`/`reconnectDelay`, `App._resuming`.
Test: `test/connectionOffline.test.js` (new, jsdom).

### 2.7 `homeDeviceId` — stage 2, `core/devicePolicy.js`

```js
export function homeDeviceId(devices, selectedDeviceId)
//   onlineStickyDeviceId(devices, selectedDeviceId) || first online device id || null
```
The one definition; used by the gate, `setHomeDevice`, `homeContext`,
`pickDevice`'s caller, the terminal socket and the composer. Test:
`devicePolicy.test.js` (new).

### 2.8 `pickDevice` — stage 2, `core/routeResolve.js`

```js
export function pickDevice(candidates, { homeDeviceId, deviceOrder })
//   candidates: rows or projects carrying deviceId; → the one on homeDeviceId,
//   else the one whose deviceId is earliest in deviceOrder (the caller passes
//   the ONLINE ids in App.devices order), else candidates[0], else null
export function resolveLegacyRoute(ref, { items, projects }, policy /* { homeDeviceId, deviceOrder } */)
//   kind "project": candidates = items ∪ projects with that project id → ref.route stamped with the picked deviceId
//   other kinds: as today, candidates narrowed by id first, then pickDevice; routes carry deviceId
```
Test: `routeResolve.test.js` (collision → home, then first online; single
match ignores policy; routes carry `deviceId`).

### 2.9 Route device prefix — stage 2, `core/router.js`

```js
function peelDevice(parts)       // parts[0]==="device" && parts[2]==="project" → { deviceId: parts[1], rest: parts.slice(2) }; else { deviceId: null, rest: parts }
function surfaceFromSegments(parts)  // today's surfaceFromHashPath body, ratchet line moved with it
function stampDevice(route, deviceId)// adds deviceId to a route that has projectId; returns route unchanged otherwise
function surfaceFromHashPath(hash)   // segments → peelDevice → surfaceFromSegments → stampDevice; complexity ≤ 3
function projectPrefix(route)        // route.deviceId ? `#/device/${e(d)}/project/${e(p)}` : `#/project/${e(p)}`
export function hashFromRoute(route) // the two project literals at :214 and :217 become projectPrefix(route); score stays 12
export function withDeviceOrResolve(route)
//   branch/issue with projectId and no deviceId → { name: "resolve", kind: "project", projectId, route }; else route
```
`app.js`: `applyRoute` and `markRoute` call `withDeviceOrResolve` first.
Test: `router.test.js` (device routes round-trip; device-less project routes
become `resolve/project` carrying the inner route; legacy unchanged;
`complexityRatchet.test.js` still 70).

### 2.10 Inbox verbs per device — stage 1, `core/inboxView.js`

```js
function verbTarget(entry)   // { call, disabled: false } from contextFor(entry.deviceId), or { call: null, disabled: "Device offline" }
```
Every `App.call` at `inboxView.js:102-623` goes through it; the menu item
renders `disabled` with that title. Replaces the eight direct reads.
Test: `inboxDom.test.js` (a foreign row's Clear calls that device's `call`).

### 2.11 Device filter — stage 3, `core/deviceFilter.js`

```js
export function filterByDevice(snapshot, deviceFilter)
//   null → snapshot (same object); id → every array filtered on row.deviceId === id,
//   devices: { [id]: snapshot.devices[id] }, cached carried over
export function rememberDeviceFilter(deviceId | null)   // App.deviceFilter + localStorage "build.deviceFilter"
```
Applied at the three feed subscriptions that paint lists (`inboxView.js:665`,
`toolbar.js:426`, and the projects face's input), never on routes. Test:
`deviceFilter.test.js` + the DOM tests the stage names.

### 2.12 `noCurrentDevice` scan — stage 3, `test/noCurrentDevice.test.js`

Same shape as `complexityRatchet.test.js:32-62`: read every `.js` under
`spa/src`, strip `//…` and `/* … */` comments, fail on
`/\bApp\.(session|call|cacheScope|chatRepository|offline|offlineSince|modelCatalog)\b/`,
and on `switchDevice`, `adoptApplicationScope`, `disposeApplicationScope`,
`resetFeedScope`, `currentCacheScope`, `setCacheDevice`, `cacheDeviceId`.
Comment stripping matters: `adoption.js:95,123` say "App.call-shaped" in
prose; reword them anyway.

### 2.13 Name-clash label — stage 1, `core/inboxProjects.js`

```js
export function clashingProjectNames(projects)   // Set of names held by projects on more than one device
```
`projectBlocks({ items, projects, devices, nowMs })` stamps `deviceId`,
`deviceName` and `clash: boolean` on each block; `projectHeadHtml` renders
`<span class="dim">deviceName</span>` after the name when `block.clash`.
Stage 2 reuses the same set in `projectMenuModel`.

## 3. Where polymorphism replaces branching

- **"Is this the home device?"** Nothing compares device ids. A creation
  surface asks `homeContext()`; a route surface asks `routeContext(App.route)`;
  a row's verb asks `contextFor(entry.deviceId)`. The three live in
  `core/deviceContexts.js` and each returns the same context shape, so a view
  is written once against `{ call, cacheScope, chatRepository, offline }`.
  Stages 1–2 keep the `App.*` aliases only so unmigrated files compile; no
  new code reads them.
- **"Which watcher hears this push?"** The watcher carries `hears(deviceId)`
  set at registration (`changeEvents.js`), so `dispatchChangeEvent` filters
  with one predicate call, not an `if` on whether the watcher named a device.
  The per-device armed state is a `Map`, and `pollIntervalMs` asks it.
- **"Which kind of route?"** `VIEWS` (`app.js:218-226`) already dispatches on
  `route.name`. The device segment is peeled by `peelDevice` before the
  existing parser and written by `projectPrefix` after it, so neither
  ratcheted router function gains a branch. Device-less work routes are
  converted in exactly one place, `withDeviceOrResolve`, and every producer
  goes through `go`/`markRoute`, which call it.
- **"Is this row foreign?"** Rows are stamped with `deviceId` and `projectKey`
  in `liveFeedSnapshot`, and every key that used a project id
  (`entryKeyOf`, block `key`, `folds`, `SCOPE_KEY`, `data-project-*`) uses
  the `projectKey` string instead. The wiring never asks "foreign?"; it asks
  the registry for the row's context and renders whatever comes back.
  The stage 1 read-only rule is a single map in `inboxView` deleted in stage 2.
- **"Online or offline?"** The context's `call` rejects while offline
  (`session.js:131-133` already does this per session through `isPaused`);
  surfaces keep their existing frozen-view treatment (`isOffline: () =>
  context.offline`). The banner asks `liveContexts().length`; the rail asks
  `context.offline`; the gate's waiting screen is the `length === 0` case.
- **"Which device's feed?"** A `Map<deviceId, snapshot>` and `mergeFeeds` —
  consumers that want one device read `feed.devices[id]`, the same shape as
  the merge, so a consumer is one function whichever it reads.
- **"Does this name clash?"** A precomputed `Set` (`clashingProjectNames`)
  consulted by the head and the toolbar menu; no per-render device comparison.
- **"Which device do terminals follow?"** One function `terminalDeviceId()`
  (route device, else home) read by `preferDeviceId` and `retargetTerminals`.
- **Cache sync per device.** `syncDeviceSnapshot(deviceId, view)` is called
  once per `snapshot.devices` entry; `activeRows` keys carry the device, so
  `refreshEntity(key)` needs no lookup of "which device was this".
- **The filter (stage 3)** is a snapshot-to-snapshot function whose `null`
  case is the identity, so no consumer branches on "is a filter set".

## 4. Seams and hazards

### Files reading the singletons today (`grep -rn "App\.\(session\|call\|cacheScope\|chatRepository\|offline\|offlineSince\)\b" spa/src`, 96 lines)

| file | count | stage that migrates it |
| --- | --- | --- |
| `connection.js` | 24 | 1 (rewritten) |
| `app.js` | 16 | 1 (aliases), 3 (deleted) |
| `core/inboxView.js` | 8 | 1 |
| `core/taskFeed.js` | 6 | 1 |
| `core/composeView.js` | 5 | 3 |
| `views/settings.js` | 4 | 3 |
| `views/branchView.js` | 4 | 2 |
| `sheets/newRepo.js` | 4 | 3 (home) |
| `core/captureDecisionView.js` | 4 | 2 |
| `core/cacheSync.js` | 4 | 1 |
| `core/agentRail.js` | 3 | 2 |
| `terminal/manager.js` | 2 | 2 |
| `core/createWork.js` | 2 | 3 |
| `core/adoption.js` | 2 (comments only) | 3 (reword) |
| `views/issueView.js` | 1 | 2 |
| `views/archive.js` | 1 | 3 (not in any stage doc — amended) |
| `sheets/setRemote.js`, `sheets/projectSettings.js`, `sheets/clone.js`, `sheets/browser.js` | 1 each | 2 (opener passes context), 3 for clone |
| `devices.js` | 1 | 3 (picker) |
| `core/console.js` | 1 | 2 |

Readers of `currentCacheScope()` outside those counts: `console.js:155`,
`changesReview.js:97`, `gitPane.js:389`, `files.js:151`,
`worktreeReview.js:77,112`, `taskReview.js:88,101`, `agentRail.js:180` —
stage 2 (all reachable from `branchView.js:34-35` (imports), `:384`
(`renderFilesTab`) and `:421` (`mountGitPane`); `:337`/`:349` mount the
task and worktree reviews).

### Ratcheted functions in files the stages edit (`complexityRatchet.test.js` pins 70)

| function | line | score | stage | what must not happen |
| --- | --- | --- | --- | --- |
| `onSnapshot` | `core/cacheSync.js:238` | 14 | 1 | no per-device loop inside; add `syncDeviceSnapshot` |
| `projectHeadHtml` | `core/inboxProjects.js:118` | 12 | 1 | clash label is `block.clash ? … : ""` — one ternary is +1 → pull the label into `deviceTagHtml(block)` |
| `inboxRowHtml`, `captureRowHtml`, `toEntry`, `toCaptureEntry`, `entryFactsText` | `core/inbox.js:512,645,324,280,230` | 12,14,15,20,14 | 1 | no "foreign" branch; substitutions only |
| `ensureProject` | `core/captureDecisionView.js:70` | 12 | 1–2 | untouched; the feed subscription changes instead |
| `surfaceFromHashPath`, `hashFromRoute` | `core/router.js:141,209` | 28, 12 | 2 | peel/prefix helpers; literals replaced, no conditions added |
| `paint` | `core/toolbar.js:136` | 13 | 2–3 | device label and filter go into the model, not the painter |
| `mountGitPane` | `core/gitPane.js:361` | 21 | 2 | `options.cacheScope` replaces `currentCacheScope()` — no `\|\|` fallback (each `\|\|` is +1) |
| `createReviewPlug` | `core/changesReview.js:78` | 16 | 2 | same rule at `:97` |
| callbacks at `agentRail.js:859`, `branchView.js:464`, `console.js:257`, `issueView.js:113,246,539,720` | — | 11–30 | 2 | context is read at the top of the mount, once; no fallbacks |
| `toRow` | `core/archive.js:66` | 12 | 3 | stamp `deviceId` in the fetch loop (`views/archive.js`), not in `toRow` |
| `composeBoxHtml` | `core/compose.js:196` | 13 | 3 | the placeholder text is computed by the caller |
| `createRelayLink`, `connect` | `core/relayLink.js:47,108` | 13, 15 | none | do not touch relayLink |

Every `a || b` and `cond ? x : y` counts one point under eslint `complexity`;
the "no fallback" rule above is why.

### Other seams

- **Import cycles.** `devices.js ↔ connection.js` already exists;
  `app.js ↔ deviceContexts.js` will. Read `App` lazily inside functions, never
  at module top level.
- **`terminalsRideOn`** (`manager.js:70`) is called from `adoptPeerLink`
  (`connection.js:131`) for whichever session upgraded. With N peer links only
  the link of the device the terminal socket is attached to may hand over
  its term channel; the others must not, or the terminal stream rides a
  channel to the wrong machine.
- **`isPaused`** (`session.js:131`) is read on every call; per-context it must
  read the context's flag, not `App.offline`, or one offline device pauses all.
- **`App.viewingContext`** is recreated on device switch today (`app.js:60`);
  with N contexts it must be created once.
- **Direct `App.call` assignment in tests** (`inboxDom.test.js:144`,
  `taskFeed.test.js:36`): stage 1 keeps these tests green through the aliases
  (plain fields, not getters); stage 3 rewrites them to register a context.
- **`devicePickerDom.test.js:3-6`** mocks `switchDevice` from `connection.js`;
  stage 1's rename to `setHomeDevice` must update the mock; stage 3 removes it.
- **Hand-built routes** that bypass `entryRoute`/`projectRoute`:
  `branchView.js:391`, `issueView.js:34-40`, `createWork.js:115-117`,
  `toolbarModel.js:48-50`. Each must carry `deviceId` in stage 2 or `go` will
  bounce it through `resolve`.
- **Wire params stay bare.** `dismissParamsOf`, `branchFinishParams`,
  `rerouteParams`, `project.init_git`, `git.branches` all send `project_id`;
  never send a `projectKey` to the bridge.
- **localStorage keys** `build.inbox.folded`, `build.toolbar.project` change
  their value vocabulary to `projectKey`; old values read as unset. No
  migration needed.
- **`App.devices` may be empty at first paint** (the api answered after a
  push): merge order must tolerate ids not in the order (last, stable).

## 5. Stage-doc amendments (applied in place)

Stage 01 (`01-device-contexts.md`):
1. §2: `cacheScope.test.js:22-35` pins "retires the old scope before adopting
   another device"; that case must change to "leaves it live; releasing
   retires it". The doc said the contract is kept.
2. §4: `resetFeedScope` is deleted here (only caller `connection.js:174`);
   the context object needs `active()`; `taskFeed.test.js`'s "late snapshot
   from the session that was replaced" becomes "from a retired context".
3. §5: `greetBridge` must take `deviceId` (it arms per device); `onPush`
   closes over the `deviceId` given as `preferDeviceId` (relayLink only ever
   lands on that device, `relayLink.js:144-148`).
4. §6: `onSnapshot` is ratcheted at 14 — the per-device loop goes in a new
   function.
5. §7: `entryKeyOf` keys no-entity rows `branch:<project_id>:<branch>` /
   `issue:<project_id>`, not by bare project id; substitute `projectKey`
   inside the same shape. The read-only rule goes in the wiring, not
   `entryRoute` (pure, no home device, and the guard would take it from 9 to
   12). `data-project*` attributes, `blocksPainted`, `ui.folded`,
   `ui.activeProjectId` carry `projectKey`; the `blockFor` selector in
   `inboxProjectsDom.test.js:45` follows. The toolbar scope key belongs to
   stage 2, not here.

Stage 02 (`02-device-identity-routes.md`):
6. §1: `resolveLegacyRoute` for `kind: "project"` needs `projects` as well as
   `items` (a plain folder has no `items[]` row, `inboxProjects.js:91-95`).
7. §2: `branchView.js:391` and `issueView.js:34-40` build routes by hand and
   must carry `deviceId`.
8. §3: the consumer list misses the `currentCacheScope()` readers
   (`gitPane.js:389`, `files.js:151`, `changesReview.js:97`, `console.js:155`,
   `worktreeReview.js:77,112`, `taskReview.js:88,101`); they take
   `cacheScope` from the options `branchView` passes.

Stage 03 (`03-retire-device-switcher.md`):
9. Context: the model catalog is at `app.js:85-108`, not 71–96.
10. Context: the alias-reader list after stage 2 also holds `views/archive.js:55`
    (`archived.list` — merge across `liveContexts()`, rows stamped `deviceId`,
    `toRow` in `core/archive.js:66` ratcheted; `views/archive.js` has no
    ratchet), `core/adoption.js:95,123` (comments) and `devices.js:62`.
11. §3: `resetFeedScope` is already gone (stage 1); the scan test must strip
    comments and also match `App.offlineSince` and `App.modelCatalog`.
