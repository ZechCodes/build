# Stage 05 — SPA core: the rendezvous seam per device, no terminal socket

Binding contract: spec rules 1, 2, 4, 5, 7 and "SPA after this plan" (`core/rendezvous.js`,
`core/sessionSwitch.js`, `core/peerLink.js`, `terminal/session.js`). This stage is the core
modules and their Vitest suites; `connection.js`, presence polling and the blocked
vocabulary are stage 06. Keep `connection.js` compiling against the new modules with the
smallest adapter and leave the policy change to stage 06.

## Context a cold agent needs

The SPA is multi-device: one context per paired device (`core/deviceContexts.js`, shape
:58-91 — `deviceId, session, call, rpc, cacheScope, chatRepository, adapter, apiVersion,
offline, offlineSince, peerLink, reconnect`), one `openRelaySession` — hence one relay
socket — per online device (`connection.js:43-77` `relayDial`/`openDeviceSession`), plus one
terminal socket for the tab (`terminal/manager.js:16,61-83`) and a transient one for the
device settings page (`connection.js:82-89`). Read `HANDOFF.md` "2026-09-14 — Multi-device"
(:287-438) first.

- `spa/src/core/relayLink.js` (257): `createRelayLink` :48; `connect` :110-221 (token, socket,
  `relayInbox` observer forwarding `device_key`/`device_offline` :126-129, **wait for
  `device_key`** :145-151 — the relay no longer sends it after stage 02, pinned key :155-165,
  re-attach rule :167-174, `session_init` :169-183, accept-or-`device_offline` :187-193,
  carrier :194-199); `lost` backoff :93-107; API :223-255. Ratchets :48 and :109.
  `relayInbox.js` is reusable.
- `spa/src/core/session.js` (188): `openRelaySession` :56-187 composes switch :87-94,
  relayLink :96-121 (`onSession` mints `createSessionRpc` :109-119), `rawCall` :138-147
  (pause check skips signaling; `wireFor`), interface :149-186 (`deviceId, call,
  installAdapter, adapter, onPush, peer, onCarrier, close`).
- `spa/src/core/sessionSwitch.js` (87): `carrying = peer ?? relay` :10, `isSignaling` :15,
  `relay()` :44-54, `peer()` :55-59, `wireFor` :72-76 (signaling waits for the relay),
  `close` :78-84.
- `spa/src/core/carrier.js` (172): `openCarrier` :27-29, `relayCarrier` :88-103,
  `channelCarrier` :105-171. `spa/src/core/peerLink.js` (155): `openPeerLink` :37-94,
  `offer` :97-102, `bothOpen` :108-136, `watchForFailure` :139-147, `restart` :151-154.
- `spa/src/terminal/session.js` (641): own switch :112-119 and own relayLink :123-138;
  `peer` :148-150; `simulateDrop` :413-414; `_relayChanged` :453-468; `_reattachAll`
  :476-501; `_watchLiveness` :603-622. `terminal/manager.js` (136): `terminalDeviceId`
  :49-52, `terminalManager` :61-83, `terminalsRideOn` :96-99, `retargetTerminals` :112-117,
  `followTerminalDevice` :130-135 (hands over `contextFor(id)?.peerLink?.term`).
- Tests encoding the old contract (rewrite, keep coverage): `relayLink.test.js` (:156 re-attach,
  :227 presence of every device, :244), `session.test.js` (:254 waits for device_key, :297,
  :388, :492-700 two-carrier block: :519, :539, :628, :667), `sessionSwitch.test.js` (:108, :117),
  `terminalCarrier.test.js` (whole file: relay + channel), `terminal.test.js` handshake
  suites, `terminalManager.test.js` (:105 drop socket once, :172/:184 hand-over),
  `peerLink.test.js`, `carrier.test.js`. Shared fake: `test/deviceSessionFixture.js`
  (`fakeSession` = `{deviceId, call, close, peer, onCarrier, onPush}` — keep that shape).
  `spa/test/complexityRatchet.test.js:46` `RATCHETED_FUNCTIONS = 61`.
  `spa/test/noCurrentDevice.test.js` bans the retired singletons — do not reintroduce any.
- Rules: eslint complexity 10, no new ratchet; Vitest first; `npm run lint && npm test &&
  npm run build`; semgrep + gitleaks before commit; commit per module.

## What to build

1. **`core/rendezvous.js`** — `createRelayRendezvous({deviceId, relayUrl, transport,
   WebSocketImpl, fetchToken, getPinnedDeviceKey, openTimeoutMs, acceptTimeoutMs})` →
   `{ open(), mint({sessionId?, sessionKeyB64?}), signalCarrier(sessionId), close(),
   isOpen(), onClosed(fn) }`. `open()` = token → socket → `authenticate` (idempotent while
   open; a `mint`/`signalCarrier` on a closed rendezvous calls it). `mint` seals to the
   api-pinned key (**no `device_key` wait**), sends `session_init` with `route_to`, resolves
   on `session_accept` or rejects on timeout ("device did not answer") — a re-attach
   passes the existing id+key. `signalCarrier` is a `relayCarrier` for that session on
   the shared socket. No background reconnect: a socket that drops mid-negotiation
   rejects what was in flight and `onClosed` fires; the caller decides. Delete
   `relayLink.js`; retire both ratchets; `RATCHETED_FUNCTIONS` 61 → 59 in the same commit.
2. **`core/sessionSwitch.js`** — the peer slot is the only active carrier (`carrying =
   peer`); `relay(carrier)` becomes `signaling(carrier)`; `wireFor(method)`: `rtc.*` → the
   signaling carrier, else the peer carrier, else a pending promise queued until
   `peer(carrier)` (resolve) or `fail(error)` (reject all). `onIdle` fires when the peer
   leaves. Rewrite `sessionSwitch.test.js` to the new rules.
3. **`core/session.js`** — `openRelaySession` → `openSession({rendezvous, deviceId, isPaused,
   onLost, onPush, ...})`: `mint` → `createSessionRpc` → switch, with the same returned
   interface plus `reattachSignaling()` (re-mint over the rendezvous with the same id+key
   and hand `signalCarrier` to the switch) and `fail(error)`. `call` keeps bypassing the
   pause for signaling. `onLost` is no longer driven by the relay socket — only by
   `onIdle` (peer gone) — because the relay is not a carrier.
4. **`core/peerLink.js`** — `openPeerLink({..., onConnected, onFailed})`: `onConnected()` after
   `bothOpen` and after every successful restart; on `failed`, `await onFailed()` (caller
   reopens the rendezvous and re-attaches signaling) then `restart`. Unchanged otherwise.
5. **`terminal/session.js`** — `TerminalSocket` no longer builds a relay link. It is given a
   session by `adoptTerminalSession(session)` (a session minted through the followed
   device's rendezvous by stage 06's `followTerminalDevice`) and a wire by `peer(carrier)`;
   `_reattachAll` on `onActive`, `_reportLost` on `onIdle`, `whenConnected` resolves on the
   first `onActive`; delete `_relayChanged`, `dropSocket`, `simulateDrop`. `manager.js`:
   `retargetTerminals(wanted)` becomes "adopt `wanted`'s terminal session" (stage 06 supplies
   the mint; here accept an injected `mintTerminalSession(deviceId)` and keep the current
   follow rules :112-135 — route device if opened here, else home; never onto a device that
   `!canAnswer`).
6. Tests: rewrite the suites above to the new contract and add: mint of two sessions on
   one rendezvous; `close()` ends every signaling carrier; a mint on a closed rendezvous
   reopens it; `wireFor` queues before the peer arrives and rejects on `fail`; the terminal
   socket re-attaches when handed a session + carrier and reports lost when the carrier
   goes, with no relay involved.

## Done when

`npm run lint && npm test && npm run build` green; `relayLink.js` gone;
`grep -rn "device_key\|device_offline\|device_online" spa/src` empty except
`connection.js`/`devices.js` (stage 06); no `RETIRED` name from `noCurrentDevice.test.js`
reappears; commits landed.
