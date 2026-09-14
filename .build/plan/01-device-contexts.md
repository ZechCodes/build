# Stage 01 — Device contexts and the merged feed: every online device, one inbox

## Goal

Open a session to **every** online device, keep them all open, poll each one's
`board.list` + `project.list`, and show the merged rows in the inbox and the
projects rail. Rows from a device other than the home device are visible and
their inbox menu verbs (seen, mute, clear, Done) work against **their** device;
opening one is not yet possible — routes carry no device until stage 2, so those
rows render `inbox-unroutable` (the class exists) with the title "Opens once
this page can name its device". The device picker is untouched: picking a
device still switches the home device (which is now only where creation goes).

Binding design: `.build/plan/00-multi-device-design.md`. Read it first.

## Context a cold agent needs

- **Sessions today.** `spa/src/connection.js`: `openAppSession` (≈41) opens one
  relay session with `preferDeviceId`; `adoptSession` (≈162) stores it on
  `App.session`, calls `adoptApplicationScope` (`app.js:53`), resets the feed on
  a device change, flushes queued captures, and starts the WebRTC upgrade
  (`upgradeToPeer`, module-level `peerLink` ≈86). `goOffline` / `resume`
  (≈196–250) are global. `openDeviceSettingsSession` (≈66) is the existing
  second-session precedent.
- **The gate** (`spa/src/views/gate.js`): `enterApp` opens one session, then
  `greetLiveBridge(); startFeed(); startCacheSync(); initInboxRail(); initToolbar(); render()`.
  `watchForOnline` polls `/api/devices` every 3 s while nothing is online.
- **Presence pushes.** `openRelaySession`'s `onDeviceKey` / `onDeviceOffline`
  land in `spa/src/devices.js` `markDeviceOnline` / `markDeviceOffline` for
  every device the account owns, whichever session heard them.
- **Cache scope** (`spa/src/core/cacheScope.js`) is a module singleton; a
  second `adoptCacheScope(otherId)` disposes the first, which invalidates every
  in-flight read of the first device.
- **Feed** (`spa/src/core/taskFeed.js`): `tick()` reads `App.call`, builds one
  snapshot via `liveFeedSnapshot`, `ownsFeedContext` drops an answer that
  arrived after a switch. `subscribeFeed` delivers `{items, plans, runs,
  externalWorktrees, pending, primaryChanges, projects}`; every consumer
  (`inboxView`, `toolbar`, `cacheSync`, `captureDecisionView`, `createWork`,
  `branchView`'s row lookups) reads those arrays.
- **Change events** (`spa/src/core/changeEvents.js`): one `armed` flag, one
  `watchers` set; `greetBridge(call, {isCurrent, onGreeting})` arms and
  refetches; `dispatchChangeEvent(payload)` is called from the session's
  `onPush` in `openAppSession`.
- **Cache sync** (`spa/src/core/cacheSync.js`): `syncContext()` reads the
  singletons; `activeRows` / `entityWatchers` are keyed by bare entity id; the
  feed record is written under `{deviceId, entityId: "", kind: "feed"}`.
- **Inbox wiring** (`spa/src/core/inboxView.js`): eight `App.call` sites
  (`entity.seen` ≈104, `capture.reroute` ≈540, `entity.mute` ≈578,
  `entity.seen`/`entity.dismiss` ≈607–609, `plan.archive`/`branch.finish`
  ≈622–623). `entryKeyOf` (`inbox.js` ≈200) keys entity rows by entity id and a
  primary row by project id; `projectBlocks` keys blocks `project:<id>`.
- **Tests to copy from:** `spa/test/appScope.test.js` (scope adoption),
  `spa/test/cacheScope.test.js`, `spa/test/taskFeed.test.js`,
  `spa/test/changeEvents.test.js`, `spa/test/cacheSync.test.js`,
  `spa/test/inboxDom.test.js`, `spa/test/inboxProjects.test.js`,
  `spa/test/devicePickerDom.test.js`. jsdom + vitest; sessions are plain
  objects `{ deviceId, call, close, peer, onCarrier }`.

## What to build

### 1. `core/deviceKey.js` (pure) and the stamped snapshot

`deviceKey`, `splitDeviceKey` per the design. In `taskFeed.js`,
`liveFeedSnapshot(board, projectList, deviceId)` stamps every row of every
collection (`items`, `plans`, `runs`, `externalWorktrees`, `pending`,
`primaryChanges`) with `deviceId`, and every project with `deviceId` and
`projectKey`. Rows also get `projectKey` when they name a `project_id`. Wire
fields are not renamed.

### 2. `core/deviceContexts.js`

The registry from the design (§5). `adoptDeviceSession(session)` does for one
device what `adoptSession` + `adoptApplicationScope` do today: same device →
retarget `call` and `chatRepository` and keep the scope; new device → create a
scope and repository. `retireDeviceContext` disposes the scope and repository
and closes the session. `cacheScope.js` gains `scopeFor(deviceId)` backed by a
map and stops disposing other devices' scopes; `adoptCacheScope` /
`currentCacheScope` / `clearCacheScope` keep their contract for the **home**
device (they are the compatibility surface the aliases in §5 ride on) — except
that adopting no longer disposes the previous device's scope: `cacheScope.test.js`
"retires the old scope before adopting another device" becomes "leaves it live;
`releaseScope` retires it" (amended per `04-primitives.md` §5.1). Keep
`appScope.test.js` green by routing `adoptApplicationScope` through the new
registry for the home device.

### 3. `connection.js`: N sessions, per-device offline

- `openDeviceSessions()`: for every device in `App.devices` with
  `status === "online"` and no live context, `openAppSession({ preferDeviceId,
  waitForDevice: false })` concurrently; each success → `adoptDeviceSession`,
  `greetLiveBridge(context)`, `upgradeToPeer(context)`. A failure marks that
  device's context offline and schedules its resume. Nothing waits on the
  slowest device.
- `markDeviceOnline` (devices.js) additionally calls `openDeviceSessions()`
  — a device coming online after boot joins without a reload.
- `goOffline(deviceId)` / `resume(deviceId)`: per context, with the backoff
  timer per context. `resume` opens with `preferDeviceId: deviceId,
  waitForDevice: true` (relayLink accepts only the matching `device_key`).
- `peerLink` becomes `context.peerLink`; `dropPeerLink(context)`.
  `terminalsRideOn` follows the home device's link (design §9 lands in stage 2).
- The global banner: shown only when `liveContexts().length === 0`. Text stays
  `offlineBannerText(name, since)` for one offline device; "All devices are
  offline" otherwise (add to `core/text.js`).
- `switchDevice(deviceId)` becomes `setHomeDevice(deviceId)`: remembers the
  choice, opens that device's session if it has none, re-points the aliases,
  `retargetTerminals()`, `render()`. It closes nothing.
- `flushCaptures()` runs when the **home** device's context becomes live.

### 4. Feed: one poller per device, one merged snapshot

`taskFeed.js` keeps a `Map<deviceId, snapshot>`; `tick(context)` writes that
device's entry (guarded by `context.active()` instead of `ownsFeedContext`);
`merged()` concatenates in `App.devices` order and is what `subscribeFeed`
delivers, with one extra field `devices: { [deviceId]: snapshot }` for
consumers that need one device's view (the toolbar and capture decision page in
stage 1 read `devices[homeId]`, so their behaviour is unchanged). `startFeed` /
`stopFeed` iterate contexts; a retired context's entry is dropped and a merge
delivered. `resetFeedScope` goes away here (its only caller is `connection.js:174`;
stage 3 does not delete it again). The context carries `active()`, and
`taskFeed.test.js`'s "late snapshot from the session that was replaced" case
becomes a late answer from a retired context (amended per `04-primitives.md` §5.2).

### 5. Change events tagged by device

`armChangeEvents(greeting, deviceId)`, `changeEventsArmed(deviceId)`,
`pollIntervalMs(fastMs, deviceId)`. `watchChanges` accepts `deviceId`; a
watcher with neither `entity` nor `deviceId` is delivered on **any** device's
`board.changed` (the merged inbox is such a watcher). `dispatchChangeEvent(payload,
deviceId)` delivers `board.changed` to that device's board watchers plus the
any-device ones; `entity.changed` matches by entity id as today.
`greetBridge(call, { deviceId, isCurrent, onGreeting })` gains `deviceId` (it
arms per device) and is called per context with
`isCurrent: () => contextFor(id)?.session === session`. `openAppSession`'s
`onPush` closes over the `deviceId` it was opened with — relayLink lands only
on `preferDeviceId` (`relayLink.js:144-148`) (amended per `04-primitives.md` §5.3).

### 6. Cache sync per device

`syncContext(context)`; the lock is still one per browser; `activeRows` keys
become `${deviceId}|${entityId}`; each device's feed record is written from its
own snapshot; the boot paint reads every known device's cached feed and merges
them the same way. `evictEntity` is already per device. `onSnapshot` is
ratcheted at 14 (`cacheSync.js:238`): the per-device loop goes in a new
`syncDeviceSnapshot(deviceId, view)`, not inside it (amended per `04-primitives.md` §5.4).

### 7. Inbox and projects rail show the merge

- `entryKeyOf`: entity rows unchanged (uuids); no-entity rows keep their shape
  with `projectKey` in place of `project_id` (`issue:<projectKey>`,
  `branch:<projectKey>:<branch>` — the key was never a bare project id; amended
  per `04-primitives.md` §5.5); captures unchanged.
- `projectBlocks`: `key: project:${projectKey}`, `id` stays the bare project id,
  block gains `deviceId` and `deviceName` (from `App.devices`, passed in as
  `devices`); `projectsNamed` folds by `projectKey`. When two blocks share a
  name across devices, the head shows the device name after the project name
  in `.dim` — only then, so the single-device rail looks exactly as it does now
  (`inboxProjectsDom.test.js` pins that). The `data-project`,
  `data-project-fold/open/create` attributes, `blocksPainted`, `ui.folded` and
  `ui.activeProjectId` carry `projectKey` — two `proj-1` blocks must not share
  a selector — and `inboxProjectsDom.test.js:45`'s `blockFor` follows.
- Fold state (`folds`) is keyed by `projectKey` (the toolbar scope key is
  stage 2's).
- The temporary read-only rule lives in `inboxView.js` as one map over the
  entries (route → `null`, title set) for rows whose `deviceId` is not the
  home device; stage 2 deletes it. Not in `entryRoute` (pure, no home device,
  at 9 — the guard would take it over the cap) and not in the ratcheted
  `inboxRowHtml` (amended per `04-primitives.md` §5.5).
- Every `App.call` in `inboxView.js` becomes `contextFor(entry.deviceId).call`;
  a missing or offline context disables the menu item with "Device offline".

### 8. Gate

`enterApp` opens sessions for every online device via `openDeviceSessions()`,
waits for the **first** to succeed (that one becomes the home device if the
sticky one is not online), and continues exactly as today.

## Tests (write first)

- `deviceKey.test.js`: mint/split round trip; a project id containing `/`
  cannot exist (bridge mints `proj-<n>`), assert split on the first `/`.
- `deviceContexts.test.js`: adopt two sessions → two contexts; re-adopt one →
  same scope and repository, new call; retire → scope inactive, session closed,
  other context untouched.
- `taskFeed.test.js`: two contexts answer → merged snapshot stamped with device
  ids in `App.devices` order; a late answer from a retired context is dropped;
  `devices[id]` holds the per-device view.
- `changeEvents.test.js`: `board.changed` from device B refreshes B's feed
  watcher and the merged inbox watcher, not A's; armed state is per device.
- `connectionOffline.test.js` (new, jsdom): device A lost → A's rows keep
  rendering greyed, banner hidden; both lost → banner shown.
- `inboxProjectsDom.test.js`: two devices each with `proj-1` render two blocks
  with distinct keys; device name shown only on the name clash.
- `inboxDom.test.js`: a foreign-device row is unroutable; its Clear calls that
  device's `call`.

## Verify

`cd spa && npm run lint && npm test && npm run build`, `semgrep`, `gitleaks`.
Browser pass with two bridges if the environment allows (design doc, last
paragraph); with one bridge, confirm nothing changed visually.

## Not in this stage

Opening a foreign-device row; device-bearing routes; the picker's meaning;
terminals following the route; removing the `App.*` aliases.
