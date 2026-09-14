// The app's E2EE session, as everything above the wire holds it.
//
// Three primitives and nothing else: `RelayLink` owns the relay socket (the
// handshake that mints this session, and the reconnect that keeps it),
// `SessionRpc` owns the key, the frames and the pending calls, and
// `SessionSwitch` owns which carrier is riding. What is written here is the
// interface the app calls them through — `{ deviceId, call, peer, onCarrier,
// close }` — and nothing about a socket. Which wire a call rides is the
// switch's rule, asked once, in `call`.
//
// The socket the handshake ran on is this session's FIRST carrier, not its only
// one: `peer(carrier)` hands it a DataChannel to ride instead, and the session
// ends when its last carrier is gone (see sessionSwitch.js). A relay socket
// lost under a live channel is not the end of anything: the link reconnects in
// the background and re-presents the same session (spec §SPA carrier and
// migration policy, 6).

import { createRelayLink } from "./relayLink.js";
import { createSessionRpc, DEFAULT_RPC_TIMEOUT_MS } from "./sessionRpc.js";
import { createSessionSwitch, isSignaling } from "./sessionSwitch.js";

export { DEFAULT_RPC_TIMEOUT_MS };

/** Whether this rejection is that timer rather than a refusal — the difference
 *  between "the daemon said no" and "the daemon has not said yet". This
 *  module's own, deliberately: `replyOrNothing` below is the one answer callers
 *  get, so no call site can re-derive the rule and reach a different verdict. */
const rpcTimedOutAfterHandoff = (error) => Boolean(error && error.timedOut && error.uncertain);

/**
 * The reply, or nothing when the browser stopped waiting for it.
 *
 * The daemon answers a mutation as soon as its own state change is durable and
 * runs the git behind that answer, so a mutation can land after this timer has
 * fired. The record is on the board either way and the push brings it, so a
 * caller that has nothing left to do with the reply carries on with null. A
 * refusal is the daemon saying no and still raises.
 */
export async function replyOrNothing(pending) {
  try {
    return await pending;
  } catch (error) {
    if (rpcTimedOutAfterHandoff(error)) return null;
    throw error;
  }
}

/** The third argument of `session.call`: the timeout alone, as every caller
 *  has always passed it, or `{ timeoutMs, priority }` — a cache warm-up names
 *  `priority: "background"` there (wire spec step 1.4). */
function callOptions(options) {
  if (typeof options === "number") return { timeoutMs: options };
  return options && typeof options === "object" ? options : {};
}

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
  /** The API adapter the last greeting selected (wire spec step 2.5), or
   *  null before one has, and for a bridge no adapter here speaks to. */
  let adapter = null;

  /** Nothing is carrying this session any more. The caller hears it once. */
  const severSession = () => {
    if (severed) return;
    severed = true;
    rpc?.fail(new Error("your device went offline"));
    onLost();
  };

  const carrierSwitch = createSessionSwitch({
    session: {
      rideOn: (carrier) => rpc?.rideOn(carrier),
      readFrom: (carrier) => rpc?.readFrom(carrier),
    },
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
      if (severed) return; // this session ended; its caller is opening another
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

  /**
   * One RPC over whichever wire this method belongs on — the switch's rule,
   * not this module's.
   *
   * Signaling runs whether or not the app is paused: the pause holds the
   * user's actions back, and `rtc.*` is the machinery that looks for a
   * better wire under them.
   */
  const rawCall = (method, params = {}, options = {}) => {
    if (isPaused() && !isSignaling(method)) {
      return Promise.reject(new Error("your device is offline — reconnecting…"));
    }
    const { timeoutMs = DEFAULT_RPC_TIMEOUT_MS, priority } = callOptions(options);
    return rpc.call(method, params, { timeoutMs, priority, carrier: carrierSwitch.wireFor(method) });
  };

  return {
    deviceId: link.deviceId(),
    /** The raw rpc through the installed adapter, when there is one: every
     *  refusal a caller sees is then an `ApiError` with a code, whichever
     *  1.x bridge answered. Before a greeting, the raw rpc. */
    call: (method, params = {}, options = {}) =>
      adapter ? adapter.call(method, params, options) : rawCall(method, params, options),
    /**
     * Install what `selectAdapter` picked for this session's bridge. The
     * adapter is bound to the raw rpc, never to `call`, so its normalisation
     * wraps the wire exactly once. A selection naming a side as `unsupported`
     * installs nothing. Returns the adapter now installed, or null.
     */
    installAdapter: (selection) => {
      adapter = selection && !selection.unsupported ? selection.create(rawCall) : null;
      return adapter;
    },
    /** The adapter installed on this session, or null. */
    adapter: () => adapter,
    /** Subscribe to what the bridge says without being asked — the upgrade's
     *  own trickled candidates among it. Returns the unsubscribe. */
    onPush: (fn) => rpc.onPush(fn),
    /** Ride this DataChannel instead of the relay, or `null` to fall back. */
    peer: (peerCarrier) => carrierSwitch.peer(peerCarrier),
    /** What re-establishes this session on a carrier it has just taken —
     *  `session.hello` and a read of every mounted surface. Registered after
     *  the session is handed over, so the first relay attach is the caller's
     *  own greeting, not a second one. */
    onCarrier: (fn) => (onCarrierChange = fn),
    /** Sever this session deliberately — the device was let go of, or a newer
     *  session for it landed and this one lost the race — with no onLost. */
    close: () => {
      carrierSwitch.close();
      severed = true;
      rpc.close(new Error("session closed"));
      link.close();
    },
  };
}
