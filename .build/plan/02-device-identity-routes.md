# Stage 02 — Composite identity and device-bearing routes: open any row from any device

## Goal

Every surface that today asks "which project" now asks "which device, which
project". Routes carry the device; every view, sheet and rail takes its RPC
caller, cache scope and chat repository from the route's device context; the
read-only rule from stage 1 is deleted and a row from any device opens.
Device-less and legacy URLs still land where they point.

Binding design: `.build/plan/00-multi-device-design.md` §3, §4, §6, §9.

## Context a cold agent needs

- **Router** (`spa/src/core/router.js`): `surfaceFromHashPath` (ratchet at 28,
  cap 10 — it must not grow; peel the device prefix off in a new small
  function and hand the remaining segments to the existing parser, then stamp
  `deviceId`), `hashFromRoute` (ratchet at 12 — same rule). `routeFromHash`
  splits the `?path=` query first. Tests: `spa/test/router.test.js`.
- **Resolve** (`spa/src/core/routeResolve.js`, `spa/src/views/resolving.js`):
  legacy ids are looked up in the feed's `items[]`; the route named `resolve`
  waits for the feed and rewrites the hash.
- **Route producers:** `entryRoute` (`core/inbox.js` ≈172), `projectRoute`
  (`core/projectModel.js`), `createWork.js:116`, `captureDecisionView.js`,
  `toolbarModel.js` / `toolbar.js` (`SCOPE_KEY` localStorage), the console
  (`core/console.js:194` calls `branch.get`), `goFromInbox` (`inboxShell.js`).
- **Route consumers holding a caller:** `views/branchView.js` (`callRpc =
  App.call` ≈220, `branchScope` ≈66, adoption ≈226, project.init_git ≈402),
  `views/issueView.js:24`, `core/captureDecisionView.js:260`,
  `core/agentRail.js` (≈180–190: `cacheScope`, `chatRepository`, `call` from
  `App`), `core/console.js`, `core/adoption.js` (call injected),
  `sheets/projectSettings.js`, `sheets/setRemote.js`, `sheets/browser.js`,
  `sheets/newRepo.js` (guards on `App.session`), `sheets/clone.js`,
  `core/createWork.js:99,341`.
- **Terminals:** `spa/src/terminal/manager.js` `preferDeviceId` (≈45) and
  `retargetTerminals` (≈80); `terminalsRideOn` takes a peer carrier.
- **Feed rows** now carry `deviceId` and `projectKey` (stage 1);
  `feed.devices[deviceId]` is a per-device view.
- `App.route` is the parsed route; `go(route)` / `markRoute(route)` in
  `app.js`; `render()` dispatches on `route.name`.

## What to build

### 1. Routes

- Parse `#/device/<d>/project/<p>/…` and `#/device/<d>/settings`: strip
  `device/<d>` when the third segment is `project`, parse the rest as today,
  stamp `deviceId: d`. `#/device/<d>` alone → that device's settings (today's
  behaviour).
- `hashFromRoute` writes the device segment for `branch` and `issue` routes
  when `route.deviceId` is set. A `branch`/`issue` route **without** a device
  is written device-less (never invent one) — and `go()` treats such a route
  as `resolve` (below), so no producer can land on a device-less work surface.
- Device-less `#/project/<p>/branch/…` and `#/project/<p>/issue/<id>` parse to
  `{ name: "resolve", kind: "project", projectId, route: <the parsed route> }`.
  Legacy resolve routes are unchanged in shape and gain nothing.
- `resolveLegacyRoute(ref, { items, projects }, policy)` now searches merged
  `items` (which carry `deviceId`) and, for `kind: "project"`, `projects` too
  — a plain folder has no `items[]` row (`inboxProjects.js:91-95`; amended per
  `04-primitives.md` §5.6) — and returns routes with `deviceId`. For `kind: "project"` and any
  legacy ref whose candidate rows span several devices: prefer the home
  device, else the first online device in `App.devices` order — a pure
  function `pickDevice(candidates, { homeDeviceId, deviceOrder })` in
  `routeResolve.js`, tested alone.

### 2. Route producers carry the device

`entryRoute(item)` uses `item.deviceId`; delete the stage 1 read-only rule and
its title. `projectRoute(project)` uses `project.deviceId`. `createWork`
routes with the device it was scoped to. The toolbar's scope
(`SCOPE_KEY`) stores a `projectKey`; `toolbarModel.projectMenuModel` lists
merged projects and shows the device name in `.dim` on a name clash (same rule
as the rail). The console's `branch.get` and `goFromInbox` pass the device
through. Routes built by hand — `branchView.js:391` (`markRoute` for the Files
tab) and `issueView.js:34-40` (`syncHash`) — carry `deviceId` too, or `go`
bounces them through `resolve` (amended per `04-primitives.md` §5.7).

### 3. Route consumers take their context from the route

A helper in `core/deviceContexts.js`:
```js
routeContext(route)  // contextFor(route.deviceId) or null
```
and in each view/sheet the first line becomes `const context =
routeContext(App.route)`, then `callRpc = context.call`, `cacheScope =
context.cacheScope`, `chatRepository = context.chatRepository`,
`isOffline = () => context.offline`. Where a sheet is opened from a row or a
project (project settings, set remote, browser, init git, adoption), the
opener passes the device's context in; `sheets/newRepo.js` and
`sheets/clone.js` take the **home** context (creation goes home — design §6)
and say so in their title ("New repository on <device name>"). A route whose
device has no live context renders the existing offline treatment of that
view, with the banner text naming the device.

`agentRail.js` reads `context.cacheScope` / `context.chatRepository` from what
the view hands it (it already accepts `context.cacheScope` and
`context.chatRepository` — make the fallback to `App.*` go away here). The
`currentCacheScope()` readers — `gitPane.js:389`, `files.js:151`,
`changesReview.js:97`, `console.js:155`, `worktreeReview.js:77,112`,
`taskReview.js:88,101` — take `cacheScope` from the options `branchView`
passes them, with no `||` fallback: `mountGitPane` and `createReviewPlug` are
ratcheted (amended per `04-primitives.md` §5.8).

### 4. Cache sync and read coordination

`readRequests.rpcReadKey` already takes `deviceId`; make every caller pass the
route context's id (grep `rpcReadKey(` and `coordinatedRead(`). Cache
addresses (`conversationCache`, `surfacesCache`, `fileDiffs`, `gitPane`,
`activityRuns`) already take `deviceId` from the scope — confirm by test that a
branch view for device B never reads or writes under device A's key.

### 5. Terminals follow the route

`preferDeviceId: () => App.route.deviceId || App.selectedDeviceId || null`;
`render()` calls `retargetTerminals()` when the route's device changed since
the last render; `terminalsRideOn` follows the peer link of the device the
socket is attached to (`contextFor(socket.deviceId)?.peerLink`).

### 6. `devicePolicy.js`

`onlineStickyDeviceId` unchanged; add `homeDeviceId(devices, selectedDeviceId)`
= sticky if online, else first online, else null — the one definition of the
home device, used by the gate, `setHomeDevice`, `pickDevice`, and the composer.

## Tests (write first)

- `router.test.js`: device routes parse and round-trip for branch (both tabs,
  slashed names, `?path=`), issue (with stage), settings; device-less project
  routes become `resolve/project` carrying the inner route; legacy URLs
  unchanged; `surfaceFromHashPath` / `hashFromRoute` complexity scores do not
  rise (`complexityRatchet.test.js` already pins the list).
- `routeResolve.test.js`: a `proj-1` collision picks the home device, then
  the first online device; a single match ignores the policy; resolved routes
  carry `deviceId`.
- `inbox.test.js` / `inboxProjects.test.js`: `entryRoute` and block routes
  carry `deviceId`; no row is unroutable for being foreign.
- `branchViewDom.test.js`, `issueView` tests, `captureDecisionDom.test.js`:
  the view calls the route device's `call`, not the home device's; an offline
  route device renders the offline state naming that device.
- `toolbarModel.test.js`: merged project menu, name-clash device label, scope
  key round-trips a `projectKey`.
- `terminal` manager test: route change to another device drops the socket
  once; same device does not.

## Verify

`npm run lint && npm test && npm run build`; `semgrep`; `gitleaks`. Browser
pass: open a branch on device B from the merged rail, confirm Changes, Files,
the agent rail and the console all talk to B (bridge logs), reload the URL,
paste a device-less `#/project/proj-1/branch/main/changes` and confirm it
resolves to the home device.

## Not in this stage

The picker's meaning and the `App.*` aliases (stage 3); anything in `bridge/`.
