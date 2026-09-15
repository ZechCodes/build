# Multi-device projects and inbox — the design every stage builds against

Status: plan (2026-09-14). Answers the question "what does it take to show all
projects and inbox entries from all connected devices at once, without a device
switcher". Three stages; each ships green on its own.

## What is true today

- The SPA holds exactly one live bridge session. `App.session` / `App.call` /
  `App.cacheScope` / `App.chatRepository` (`spa/src/app.js:22-25`) are
  singletons, `switchDevice` (`spa/src/connection.js:255-285`) closes the old
  session, and `resetFeedScope()` (`spa/src/core/taskFeed.js:36-39`) throws the
  other device's rows away.
- The inbox and the projects rail are pure functions over one snapshot
  (`core/inbox.js`, `core/inboxProjects.js`) that one poller fills from one
  `board.list` + `project.list` (`core/taskFeed.js:75-89`).
- Project ids collide across devices: the bridge mints `proj-<n>` from a
  per-machine counter (`bridge/src/app/projects/project_registry.rs:160`).
  Every device's first project is `proj-1`. Routes
  (`#/project/<id>/…`), `projectBlocks` keys, `routeResolve`, and the bridge's
  attention keys `row:<project_id>:…` all assume that id is unique. Run, issue
  and capture ids are uuids and do not collide.
- Routes carry no device except `#/device/<id>/settings`.
- Change pushes (`board.changed`, `entity.changed`) are per-session and
  untagged; `core/changeEvents.js` keeps one global armed flag and watcher set.
- One device going offline flips the whole app offline (`App.offline`).
- In our favour: the relay already pushes every device's `device_key` /
  `device_offline` to the client; `openDeviceSettingsSession` already proves N
  concurrent sessions work; the browser cache is already keyed
  `deviceId|entityId|kind|sub` (`core/localCache.js:91`); and the planning
  record already anticipates this ("Router = device-scoped v1 … account-wide
  fan-in is a follow-on once multi-device aggregation exists",
  `planning/v2/UX Redesign Decisions.md:151`).

## Decisions

1. **The bridge does not change.** A project's identity across the account is
   the pair `(deviceId, projectId)`; the client knows which session answered
   every row, so it stamps `deviceId` itself. Minting globally unique project
   ids on the bridge would force a persisted-id migration (config, attention
   map, entity→project bindings, capture routings) for no client benefit. It
   stays an optional follow-on.
2. **Device ids on the wire are the api's uuids** (`GET /api/devices`
   `device_id`; `session.deviceId`). They are URL-safe; routes still
   `encodeURIComponent` them like every other segment.
3. **A "device key" is the string `${deviceId}/${projectId}`** and is minted in
   exactly one place, `core/deviceKey.js`:
   ```js
   export const deviceKey = (deviceId, projectId) => `${deviceId}/${projectId}`;
   export const splitDeviceKey = (key) => { /* → { deviceId, projectId } or null */ };
   ```
   Nothing else concatenates the two. Feed rows and projects carry both
   `deviceId` and `projectKey` (stamped in `liveFeedSnapshot`, the one place the
   wire is read — `taskFeed.js:53-73`). Wire fields (`project_id`) are never
   rewritten: the bridge still needs the bare id in every RPC.
4. **Routes gain a device segment in front of the project:**
   ```
   #/device/<deviceId>/project/<projectId>/branch/<name>/<tab>
   #/device/<deviceId>/project/<projectId>/issue/<issueId>[/stage/<stageId>]
   #/device/<deviceId>/settings                       (exists today)
   #/inbox, #/capture/<id>, #/account/<page>            (unchanged: not per-device)
   ```
   The device-less forms (`#/project/…` and every legacy URL) keep parsing and
   become `resolve` routes: the app looks the project up across every device's
   feed (`core/routeResolve.js`) and rewrites the hash. One match opens it; a
   collision prefers the home device (below), then the first online device in
   `App.devices` order.
5. **A device context** replaces the four singletons. `core/deviceContexts.js`:
   ```js
   // { deviceId, session, call, cacheScope, chatRepository, offline, offlineSince, peerLink }
   contextFor(deviceId)        // or null
   liveContexts()              // every open, unpaused context, in App.devices order
   adoptDeviceSession(session) // create or retarget the context for session.deviceId
   retireDeviceContext(deviceId)
   ```
   `App.session` / `App.call` / `App.cacheScope` / `App.chatRepository` survive
   stages 1–2 as aliases of the **home device's** context so untouched
   consumers keep working; stage 3 deletes them and pins that with a test.
6. **The home device.** `App.selectedDeviceId` (localStorage
   `build.selectedDeviceId`) stops meaning "the device the app shows" and means
   "where creation goes when nothing else says": `capture.create`, New project,
   clone, new repo, and the terminal socket's default target. The sticky
   policy in `core/devicePolicy.js` (`onlineStickyDeviceId`) keeps choosing it,
   falling back to the first online device.
7. **Offline is per device.** A device's rows stay in the rail, greyed with an
   "offline" mark, and their actions are disabled; nothing is removed. The
   global banner and the gate's waiting screen appear only when **no** device
   is online.
8. **Captures stay device-scoped** (per the planning decision). The composer
   sends to the home device; the capture decision page offers that device's
   projects. Account-wide capture routing is out of scope here.
9. **Terminals follow the route**, as they follow the session today: the
   terminal socket's `preferDeviceId` reads the current route's device, else
   the home device, and `retargetTerminals()` runs on route change. One socket
   at a time; per-device terminal sockets are a follow-on.
10. **Push events are tagged at the session.** `dispatchChangeEvent(payload,
    deviceId)`; the feed registers one board-scoped watcher per device;
    entity watchers keep matching on entity id (uuids, unique). `armed` becomes
    per device.

## Out of scope (say so in the stage docs, do not do it)

Bridge-minted unique project ids; account-wide capture routing; per-device
terminal sockets; WebRTC fan-out (a non-goal in `WebRTC Transport Spec.md:310`);
any skriftapp change.

## Repo rules that apply to every stage

TDD (vitest first: `cd spa && npx vitest run`); `npm run lint` (eslint
`complexity` max 10 — **no function joins the ratchet list**, split instead);
`npm run build`; `semgrep` and `gitleaks` before every commit; commit as you
go; copy in plain words. The `Build/spa:verify` skill drives the browser pass.
A two-device browser pass needs a second bridge paired to the same account: run
a second `bridge` process with its own config directory against the local relay
(`deploy/README.md`), pair it under Settings → Devices. If that is not possible
in the environment, say so in the stage report instead of claiming the pass.
