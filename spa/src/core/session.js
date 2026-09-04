// The app's E2EE session, as everything above the wire holds it.
//
// Three primitives and nothing else: `RelayLink` owns the relay socket (the
// handshake that mints this session, and the reconnect that keeps it),
// `SessionRpc` owns the key, the frames and the pending calls, and
// `SessionSwitch` owns which carrier is riding. What is written here is the
// interface the app calls them through — `{ deviceId, call, peer, onCarrier,
// close }` — and nothing about a socket.
//
// The socket the handshake ran on is this session's FIRST carrier, not its only
// one: `peer(carrier)` hands it a DataChannel to ride instead, and the session
// ends when its last carrier is gone (see sessionSwitch.js). A relay socket
// lost under a live channel is not the end of anything: the link reconnects in
// the background and re-presents the same session (spec §SPA carrier and
// migration policy, 6).

import { createRelayLink } from "./relayLink.js";
import { createSessionRpc } from "./sessionRpc.js";
import { createSessionSwitch } from "./sessionSwitch.js";

const DEFAULT_RPC_TIMEOUT_MS = 12000;

export async function openRelaySession({
  relayUrl,
  transport,
  WebSocketImpl,
  fetchToken,
  getPinnedDeviceKey,
  preferDeviceId = null,
  waitForDevice = false,
  deviceWaitMs,
  acceptTimeoutMs,
  isPaused = () => false,
  onDeviceKey = () => {},
  onDeviceOffline = () => {},
  onLost = () => {},
  onPush = () => {},
}) {
  let rpc = null;
  let severed = false;
  let onCarrierChange = () => {};

  /** Nothing is carrying this session any more. The caller hears it once. */
  const severSession = () => {
    if (severed) return;
    severed = true;
    rpc?.fail(new Error("your device went offline"));
    onLost();
  };

  const carrierSwitch = createSessionSwitch({
    session: { rideOn: (carrier) => rpc?.rideOn(carrier) },
    onActive: () => onCarrierChange(),
    onIdle: severSession,
  });

  const link = createRelayLink({
    relayUrl,
    transport,
    WebSocketImpl,
    fetchToken,
    getPinnedDeviceKey,
    preferDeviceId: () => preferDeviceId,
    waitForDevice,
    deviceWaitMs,
    acceptTimeoutMs,
    carrying: () => carrierSwitch.active(),
    onDeviceKey,
    onDeviceOffline,
    onSession: (opened) => {
      // Whatever was riding the session before this one is not riding this one.
      carrierSwitch.peer(null);
      rpc = createSessionRpc({
        transport,
        ...opened,
        noCarrier: () => new Error("your device went offline"),
      });
      rpc.onPush(onPush);
    },
    onRelay: (carrier) => carrierSwitch.relay(carrier),
  });

  try {
    await link.start();
  } catch (error) {
    link.close(); // a handshake the caller is told about is not one to retry under it
    throw error;
  }

  return {
    deviceId: link.deviceId(),
    call: (method, params = {}, timeoutMs = DEFAULT_RPC_TIMEOUT_MS) =>
      isPaused()
        ? Promise.reject(new Error("your device is offline — reconnecting…"))
        : rpc.call(method, params, { timeoutMs }),
    /** Signaling is pinned to the relay carrier: `rtc.*` never rides the
     *  channel it negotiates, so an ICE restart works while the channels are
     *  down (spec §Signaling). It runs whether or not the app is paused: the
     *  pause holds the user's actions back, and this is the machinery that
     *  looks for a better wire under them. */
    signal: (method, params = {}, timeoutMs = DEFAULT_RPC_TIMEOUT_MS) =>
      rpc.call(method, params, { timeoutMs, carrier: carrierSwitch.relayCarrier() }),
    /** Ride this DataChannel instead of the relay, or `null` to fall back. */
    peer: (peerCarrier) => carrierSwitch.peer(peerCarrier),
    /** What re-establishes this session on a carrier it has just taken —
     *  `session.hello` and a read of every mounted surface. Registered after
     *  the session is handed over, so the first relay attach is the caller's
     *  own greeting, not a second one. */
    onCarrier: (fn) => (onCarrierChange = fn),
    /** Sever this session deliberately (e.g. switching devices) — no onLost. */
    close: () => {
      carrierSwitch.close();
      severed = true;
      rpc.close(new Error("session closed"));
      link.close();
    },
  };
}
