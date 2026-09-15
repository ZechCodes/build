# Stage 06 — SPA policy: live only over channels, fail closed per device, presence from the api

Binding contract: spec rules 2, 3, 4, 6 and "SPA after this plan" (`connection.js`,
`devices.js`, blocked vocabulary).

## Context a cold agent needs

Read `HANDOFF.md` "2026-09-14 — Multi-device" (:287-438) and the header of
`spa/src/connection.js` (:1-9: one session per online device; the account-wide screen only
when nothing is reachable). Current code (549 lines):

- `relayDial` :43-50, `openDeviceSession` :60-77 (`isPaused` per device, `onDeviceKey:
  markDeviceOnline`, `onDeviceOffline: markDeviceOffline`, `onLost: goOffline(deviceId)`,
  push filter), `openDeviceSettingsSession` :82-89 (a socket outside the registry),
  `upgradeToPeer` :102-119 ("staying on the relay" — the fail-open site), `adoptPeerLink`
  :123-138, `dropPeerLink` :143-150, `followTerminalsIfTheirs` :155-157, `retireDevice`
  :174-179, `greetLiveBridge` :191-219, `landSession` :227-249 (greets **before** the
  upgrade), home follow :256-319, `connectDevice` :330-342, `openDeviceSessions` :384-388,
  `guessAtStaleDevices` :401-404 (comment relies on relay pushes), `wantsSession` :420-425,
  `goOffline(deviceId)` :452-462, `resume` :467-482, `claimResumedSession` :484-493 (parks on
  the relay's `device_key` — gone), `scheduleResume` :512-516.
- `spa/src/devices.js` (221): header :1-2, `refreshDevices` :16-21, `pinnedDeviceTransportKey`
  :39-45, `markDevice` :49-59, `markDeviceOnline` :61-76 (late device joins via
  `openDeviceSessions()`), `markDeviceOffline` :89-92, picker-as-filter :96-152,
  `deviceLabel` :148-152 (`deviceAwayWord`). Creation device: `views/settings.js:105-130`.
- `spa/src/views/gate.js` (414): `connectToApp` :54-71, `holdAppWhileNoDeviceAnswers`
  :104-109 (`liveContexts().length ? leaveHold : holdForDevices`), `watchForOnline` :161-179
  (the only poll, 3 s, armed after a failed first connect), `renderWaiting` :312-331,
  `waitingSituation`/`WAITING_TEXT` :298-310. Per-device away: `core/deviceAway.js`
  (`deviceAwayText` :35-38, `deviceAwayMark` :42, `deviceAwayWord` :45),
  `core/deviceNotice.js` (`mountDeviceNotice` :41-46, `mountDeviceStrip` :76-84),
  `core/inboxDevices.js` `paintDeviceState` :87-95. `deviceContexts.js`:
  `setContextOffline(deviceId, mark)` :251-258 is the one writer of the offline mark;
  `canAnswer` :163-164; `liveContexts` :177-179.
- Tests: `spa/test/connectionOffline.test.js` (1013 lines; `openedFor(id)` counts sockets per
  device :80; :323 "the moment the relay says its bridge went", :349, :634 `waitForDevice`,
  :703 late device via `markDeviceOnline`, :745-778 stale-list guess, :1000 pause per device),
  `peerUpgrade.test.js` (:198 "stays on the relay when the ICE servers cannot be minted" —
  inverts; :178, :214, :229, :241, :263, :305, :324/:344 device-settings socket),
  `connectionGreeting.test.js`, `deviceContexts.test.js`, `devicePickerDom.test.js` (:89),
  `deviceNotice.test.js`, `iceServersApi.test.js:35` title, gate tests (`grep -l gate
  spa/test/*.js`), `terminalManager.test.js`, `routeTerminals.test.js`.
- The `Build/spa:verify` skill drives a real browser against the compose stack
  (`deploy/compose.two-bridges.yml` + `web/pair-another.mjs` for two devices — see
  `deploy/README.md` "Two bridges on one account").

## What to build

1. **The connect sequence**, per device, in `connectDevice(deviceId)`: rendezvous for that
   device (one per context; store it on the context as `context.rendezvous`) → `openSession`
   (app) → `openPeerLink({signal: session.call, onConnected: () => rendezvous.close(),
   onFailed: () => session.reattachSignaling()})` → `session.peer(link.app)` →
   `landSession(session)` (greeting now runs over the channel; the switch queue means the
   greeting call simply waits) → `followTerminalsIfTheirs`. Any throw → `blockDevice(deviceId,
   reason)`: close the rendezvous and session, drop the link, `setContextOffline(deviceId,
   blockedMark(reason))`. Reasons: `no-webrtc`, `ice-servers`, `refused`, `timeout`, `failed`,
   `lost` (a live link whose restart failed — the old `goOffline` trigger), `unreached`
   (the rendezvous mint timed out: the bridge is not on the relay).
2. **Blocked vocabulary** in `core/deviceAway.js`: `blockedMark(reason)` / `blockedText(reason)`
   (one sentence each, plain words: "This machine's direct connection could not be made:
   …"); `deviceAwayWord` shows "blocked" for a blocked mark. The device strip
   (`deviceNotice.js`) and the waiting screen (`gate.js renderWaiting`) get a **Retry**
   control that calls `connectDevice(deviceId)`. No account-wide banner: the existing
   hold-when-none-live rule covers the all-blocked case; `WAITING_TEXT` gains a
   `blocked` situation ("your machines are online but none could be reached directly").
3. **Presence** in `devices.js`: delete `markDeviceOnline/Offline`; `watchPresence()` polls
   `refreshDevices` every 15 s while the app is open (immediate on `visibilitychange` →
   visible; stopped on gate/logout), and after each refresh: `openDeviceSessions()` for
   newly online devices without a live context (late join), and for a context whose device
   is no longer `online`, `goOffline(deviceId)` with mark `deviceAwayMark`. `guessAtStaleDevices`
   and `claimResumedSession` lose their relay-push assumptions: `resume(deviceId)` waits for
   the poll to say `online` (no `waitForDevice` socket parking) and then runs
   `connectDevice` once. `wantsSession` unchanged. The gate's 3 s `watchForOnline` stays.
4. **Terminals**: `followTerminalDevice()` (manager) mints the terminal session through the
   followed device's `context.rendezvous` (`mint` reopens it if closed; close it again when
   the mint resolves and no negotiation is pending) and hands `adoptTerminalSession` +
   `peer(context.peerLink.term)`; a device with no live peer link gets nothing (shells show
   the device strip). Per-device terminal sessions stay a follow-on.
5. **Device settings session**: `openDeviceSettingsSession(deviceId)` returns the device's
   context session when it is live, else rejects with the device's away/blocked text; the
   separate socket is deleted (the bridge refuses relay RPC after stage 03).
6. Tests: `connectionOffline.test.js` and `peerUpgrade.test.js` rewritten to the new
   contract, keeping every still-true behaviour (one context per device, lost device stays
   known and away while another answers, hold when none live, security stop, home moves,
   pause per device) and adding: channels open → live and rendezvous closed; ICE servers
   unavailable → that device blocked with `ice-servers`, rendezvous closed, no non-`rtc.*`
   call ever sent over the relay (assert on the fake socket); restart failure → `lost`; Retry
   runs the sequence; the poll joins a late device and marks a gone device away; a blocked
   device does not hold the app while another is live; device-settings session reuses the
   context. Then the browser pass with `Build/spa:verify` on the two-bridge stack: both
   devices reach the board with their relay sockets closed (network panel), stopping one
   bridge greys its rows within the poll interval, making `POST /api/rtc/ice-servers` fail
   blocks a device with the reason and Retry recovers it. Record what you saw in the commit.

## Done when

`npm run lint && npm test && npm run build` green; `grep -rn "staying on the relay\|device_key\|device_offline\|markDeviceOnline\|waitForDevice" spa/src` empty; the browser pass recorded; commits landed.
