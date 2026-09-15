# Stage 05 — SPA core: the rendezvous seam, one socket for two sessions, no terminal socket

Binding contract: spec rules 1, 2, 4, 5, 7 and "SPA after this plan" (`core/rendezvous.js`,
`core/sessionSwitch.js`, `core/peerLink.js`, `terminal/session.js`). This stage is the
core modules and their Vitest suites only; `connection.js`, the blocked UI and presence
polling are stage 06. Keep `connection.js` compiling against the new modules with the
smallest possible adapter and leave the policy change to stage 06.

## Context a cold agent needs

- `spa/src/core/relayLink.js` (250 lines): `createRelayLink` :48-65 options, state :74-82
  (`session` outlives sockets), `lost` :94-106 backoff, `connect` :109-215 (token :120,
  socket :121, `relayInbox` observer :125-128 handles `device_key`/`device_offline`, waits
  for `device_key` :144-150 — **gone after stage 02, the relay no longer sends it**; pinned
  key check :154-160; re-attach :162-163; `session_init` :164-177; `session_accept` :181-188;
  carrier hand-over :194-197; tail `device_offline` :200-202), API :217-249. Two eslint
  complexity ratchets at :47 and :108. `relayInbox.js` :15-63 is reusable as-is.
- `spa/src/core/session.js` :239-264 wires `createRelayLink` into `createSessionSwitch`
  (`carrying`, `onSession` → fresh `createSessionRpc`, `onRelay`); facade :273-304 (`call`
  consults `wireFor` :286, signaling bypasses the offline pause :284).
- `spa/src/core/sessionSwitch.js`: `carrying = peer ?? relay` :10, `isSignaling` :15,
  `wireFor` :72-76 (signaling waits for the relay), `close` :78-84.
- `spa/src/core/carrier.js`: `relayCarrier` :59-74 reads `e2ee_envelope` off a socket for
  one `sessionId`; `channelCarrier` :76-142.
- `spa/src/core/peerLink.js`: `openPeerLink` :37-94, `offer` :97-102, `bothOpen` :108-136,
  `watchForFailure` :139-147, `restart` :151-154.
- `spa/src/terminal/session.js` (628): constructor :85-132 builds its **own**
  `createSessionSwitch` :106-113 and `createRelayLink` :117-131; `peer` :141-143; `start`
  :147-150; `_openSession` :415-427; `_relayChanged` :441-456; `_reattachAll` :464-489;
  `_watchLiveness` :591-610; `_reportLost` :622-627. `spa/src/terminal/manager.js` :34-57
  constructs it with `RELAY_URL` etc.
- Tests that encode the old contract (rewrite, don't delete coverage):
  `spa/test/relayLink.test.js` (presence at :224; device_key wait throughout),
  `sessionSwitch.test.js` (:108 signaling→relay, :117 waits for relay),
  `session.test.js` `:382-` "rides two carriers" (:409, :429, :518, :557),
  `terminalCarrier.test.js` (whole file assumes the terminal's own socket),
  `terminal.test.js` handshake suites :134-948, `terminalManager.test.js`,
  `peerLink.test.js`, `carrier.test.js`. `spa/test/complexityRatchet.test.js:24`
  `RATCHETED_FUNCTIONS = 70`.
- Rules: eslint complexity 10, no new ratchet; Vitest first; `npm run lint && npm test
  && npm run build`; semgrep + gitleaks before commit; commit per module.

## What to build

1. **`core/rendezvous.js`** — `createRelayRendezvous({relayUrl, transport, WebSocketImpl,
   fetchToken, getPinnedDeviceKey, openTimeoutMs, acceptTimeoutMs})` returning the
   `Rendezvous` interface: `open()` (token → socket → `authenticate`; idempotent while
   open), `mint({deviceId, sessionId?, sessionKeyB64?})` → `{sessionId, sessionKeyB64,
   deviceId, carrier}` (pinned key from the api via `getPinnedDeviceKey`, **no wait for
   `device_key`**; `session_init` with `route_to`; `session_accept` or timeout
   "device did not answer"; a re-attach passes the existing id+key), `signalCarrier(sessionId)`
   → the relay `Carrier` for that session (from `carrier.js` `relayCarrier`, one per
   session on the shared socket), `close()` (closes the socket; every carrier ends),
   `isOpen()`. No automatic reconnect loop: a socket that drops mid-negotiation rejects
   the in-flight `mint`/signal and the caller decides. Both relayLink ratchets retire with
   `relayLink.js` deleted; `RATCHETED_FUNCTIONS` 70 → 68 in the same commit.
2. **`core/sessionSwitch.js`** — the peer slot is the only active carrier
   (`carrying = peer`); `relay(carrier)` becomes `signaling(carrier)`; `wireFor(method)`:
   `rtc.*` → the signaling carrier, else the peer carrier, else a **pending promise**
   queued until `peer(carrier)` (resolve) or `fail(error)` (reject all). `onIdle` fires when
   the peer carrier leaves. Update the tests to the new rules.
3. **`core/session.js`** — `openRelaySession` → `openSession({rendezvous, deviceId, ...})`:
   `mint` → `createSessionRpc` → switch; expose `reattachSignaling()` which re-mints over
   a freshly opened rendezvous with the same id+key and hands `signalCarrier` to the
   switch; expose `fail(error)`. `call` keeps bypassing the offline pause for signaling.
4. **`core/peerLink.js`** — `openPeerLink({signal, fetchIceServers, onPush, onConnected,
   onFailed, ...})`: `onConnected()` after `bothOpen` and after every successful restart;
   on `connectionState === "failed"`, `await onFailed()` (the caller reopens the
   rendezvous) then `restart`. Unchanged otherwise.
5. **`terminal/session.js`** — `TerminalSocket` is constructed with `{session}` (the minted
   terminal session from `openSession`) instead of a URL; delete its `createRelayLink`,
   `_relayChanged`, `dropSocket`/`simulateDrop`; `peer(carrier)` is the only way it gets a
   wire; `_reattachAll` on `onActive`; `_reportLost` on `onIdle`; `whenConnected` resolves
   on the first `onActive`. `terminal/manager.js` gets `adoptTerminalSession(session)` and
   `terminalsRideOn(carrier)`; `retargetTerminals` becomes "adopt the new device's
   terminal session".
6. Tests: rewrite the suites named above to the new contract, keeping every behaviour
   that still exists (pinned-key mismatch refused, accept timeout, re-attach reuses id+key,
   chunked channel frames, liveness ping closes the channel, `_reattachAll` forgets reaped
   terminals, …) and adding: a mint of two sessions on one socket, `close()` ends both
   carriers, `wireFor` queues before the peer arrives and rejects on `fail`.

## Done when

`npm run lint && npm test && npm run build` green; `relayLink.js` gone;
`grep -rn "device_key\|device_offline\|device_online" spa/src` empty except
`connection.js`/`devices.js` (stage 06); commits landed.
