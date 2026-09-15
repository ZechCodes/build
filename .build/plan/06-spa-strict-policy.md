# Stage 06 — SPA policy: live only over channels, fail closed, presence from the api

Binding contract: spec rules 2, 3, 4, 6 and "SPA after this plan" (`connection.js`,
`devices.js`, blocked UI).

## Context a cold agent needs

- `spa/src/connection.js` (285): `openAppSession` :41-62 (`onDeviceKey`/`onDeviceOffline`
  :51-52, push filter :58-60), `openDeviceSettingsSession` :66-82, `upgradeToPeer` :97-113
  ("staying on the relay" — the fail-open site), `adoptPeerLink` :117-132, `dropPeerLink`
  :137-144, `adoptSession` :162-183 (upgrade in background :182), `restoreOnline` :185-191,
  `goOffline` :196-212, `resume` :214-252, `switchDevice` :255-285.
- `spa/src/devices.js`: header :1-2, `refreshDevices` :12-16, `pinnedDeviceTransportKey`
  :28-34, `markDeviceOnline/Offline` :36-55 (delete), `deviceLabel` :75-77,
  `selectDevice` :97-110. `spa/src/views/gate.js` `watchForOnline` :62-80 (3 s poll),
  `renderWaiting` :189-206, `boot` :209. `spa/src/core/devicePolicy.js`
  `onlineStickyDeviceId`. `spa/src/views/deviceSettings.js:119-121` gates on status.
- Offline UI: `#offbar` (`spa/index.html:27-28`), `offlineBannerText` (`core/text.js:16-19`),
  `body.offline`, `setConn` :36-39, `#conn` dot; composer gating `composeView.js:50`
  (`!App.call && !App.offline`), branch view `isOffline` probes, `surfaceTabs.js`
  `RECONNECTING_MESSAGE` :16 and `attachConnectionOverlay` :62-80. `App` fields
  `spa/src/app.js:32-37`.
- Tests: `spa/test/peerUpgrade.test.js` (:138 "stays on the relay when the ICE servers
  cannot be minted" inverts; :119, :157, :173, :186, :213, :220, :236, :260-),
  `devicePickerDom.test.js`, `devicePolicy.test.js`, `iceServersApi.test.js:35` title,
  gate tests (find with `grep -l gate spa/test/*.js`).
- The `Build/spa:verify` skill drives a real browser against the compose stack; use it
  for the visual pass at the end (blocked banner, retry, device picker status).

## What to build

1. **The connect sequence** in `connection.js`: `openAppSession({deviceId})` =
   `rendezvous.open()` → `openSession` (app) → `openSession` (terminal) →
   `openPeerLink({signal: app.call, onConnected: () => rendezvous.close(), onFailed: () =>
   reattachBoth()})` → `App.session = app`, `adoptTerminalSession(term)`, `session.peer(link.app)`,
   `terminalsRideOn(link.term)` → `greetLiveBridge()` → `restoreOnline()`. No call is
   dispatched before the channels are up (the switch queues; the queue drains on
   `peer(...)`). `reattachBoth` = `rendezvous.open()` + `app.reattachSignaling()`; the
   terminal session does not re-attach on the relay (it never signals).
2. **Blocked state.** `App.blocked = null | {reason, since}`; `enterBlocked(reason)`
   closes the rendezvous and both sessions, drops the peer link, sets `body.blocked`,
   fills `#blockbar` (new, beside `#offbar`) from `blockedBannerText(reason)` with a
   Retry button that runs `openAppSession` again; `restoreOnline` clears it. Reasons:
   `no-webrtc` (no `RTCPeerConnection`), `ice-servers`, `refused`, `timeout`, `failed`,
   `lost` (a live link whose restart failed, i.e. the old `goOffline` trigger).
   `canSend` and every `isOffline` probe treat blocked as offline. A device switch clears
   blocked and tries the new device.
3. **`goOffline` / `resume`** — `resume` no longer loops on a backoff by default: the
   device-unreachable path (rendezvous accept timeout) shows the gate's "Waiting for your
   device" screen with the 3 s presence poll, and a device coming online triggers one
   attempt. A peer link that ends after being live → `enterBlocked("lost")`, no automatic
   retry (spec rule 3). Keep the `offlineSince` stamp for the banner text.
4. **Presence.** Delete `markDeviceOnline/Offline`; `watchPresence()` in `devices.js`:
   15 s `refreshDevices` while the app is open, immediate on `visibilitychange` to
   visible, stopped on gate. `deviceLabel` unchanged. `devices.js` header rewritten.
5. **Device settings session** (`openDeviceSettingsSession`) must also ride a channel:
   reuse the app session when the selected device matches; otherwise open a full
   `openAppSession`-style link to that device (rendezvous + session + peer) and close it
   on leave. It must never run RPC over the relay (the bridge refuses it after stage 03).
6. Tests: `peerUpgrade.test.js` becomes `connectPolicy.test.js` with: channels open →
   live, rendezvous closed; ICE servers unavailable → blocked `ice-servers`, rendezvous
   closed, no relay RPC sent; connection failed during restart → blocked `lost`; retry
   from blocked runs the sequence; device switch clears blocked; presence poller cadence
   and visibility trigger; blocked banner DOM. Then the browser pass with `Build/spa:verify`
   against `deploy/compose.real.yml` (the compose QA may be red until stage 07 — drive
   the browser by hand and record what you saw in the commit message).

## Done when

`npm run lint && npm test && npm run build` green; `grep -rn "staying on the relay\|device_key\|device_offline\|markDeviceOnline" spa/src` empty; a real browser reaches the board over the DataChannels with the relay socket closed (check the network panel) and shows the blocked banner when `POST /api/rtc/ice-servers` is made to fail; commits landed.
