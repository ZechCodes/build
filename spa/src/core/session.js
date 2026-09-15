// The app's E2EE session, as everything above the wire holds it.
//
// Three primitives and nothing else: a `Rendezvous` mints the session and hands
// it the wire its signaling rides (core/rendezvous.js), `SessionRpc` owns the
// key, the frames and the pending calls, and `SessionSwitch` owns which carrier
// is riding. What is written here is the interface the app calls them through —
// `{ deviceId, call, peer, onCarrier, close }` — and nothing about a socket.
// Which wire a call rides is the switch's rule, asked once, in `call`.
//
// The session's only carrier is the peer connection (spec rules 1 and 2): the
// rendezvous carries `rtc.*` and nothing else, and closes once the channels are
// open. A session with no channel is a session nothing is carrying — there is
// no relay to fall back to — so `onLost` is the switch going idle and nothing
// else. Re-attaching signaling over a reopened rendezvous keeps the same
// session id and key: a session is minted once, not once per socket, and
// re-keying under a live channel would strand every frame in flight on it.

import { createRelayRendezvous } from "./rendezvous.js";
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

/**
 * One E2EE session with one device, over the rendezvous that found it.
 *
 * @param rendezvous that device's `Rendezvous` — the relay one today, a direct
 *   one when that mode is built. This module never learns which.
 * @param isPaused whether the user's calls are being held back. Signaling runs
 *   either way: the pause holds the user's actions, and `rtc.*` is the
 *   machinery looking for a better wire under them.
 * @param onLost nothing is carrying this session any more. The peer connection
 *   is the only thing that ever was, so this is the channel going — never a
 *   relay socket, which is not a carrier.
 */
export async function openSession({
  rendezvous,
  transport,
  deviceId,
  isPaused = () => false,
  onLost = () => {},
  onPush = () => {},
}) {
  let rpc = null;
  let severed = false;
  let onCarrierChange = () => {};
  /** The API adapter the last greeting selected (wire spec step 2.5), or
   *  null before one has, and for a bridge no adapter here speaks to. */
  let adapter = null;
  /** This session's lease on the rendezvous, while one is open. */
  let signaling = null;

  /** Nothing is carrying this session any more. The caller hears it once. */
  const severSession = () => {
    if (severed) return;
    severed = true;
    const gone = new Error("your device went offline");
    rpc?.fail(gone);
    // Nothing is coming back on this session: the caller connects again, which
    // is a new one. A call made after this is refused rather than held.
    carrierSwitch.fail(gone);
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

  /** Take this session's `rtc.*` wire off the rendezvous as it stands now. A
   *  rendezvous that closes takes the wire with it, and the switch holds the
   *  next signaling call until one is back. */
  const takeSignalingWire = (minted) => {
    signaling = rendezvous.signalCarrier(minted.sessionId);
    signaling.onClose(() => carrierSwitch.signaling(null));
    carrierSwitch.signaling(signaling);
  };

  const minted = await rendezvous.mint({});
  rpc = createSessionRpc({
    transport,
    ...minted,
    noCarrier: () => new Error("your device went offline"),
  });
  rpc.onPush(onPush);
  takeSignalingWire(minted);

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
    // Workspace detail waits until the bridge answers or the session fails;
    // every other RPC retains the ordinary browser deadline.
    const defaultTimeoutMs = method === "workspace.get" ? null : DEFAULT_RPC_TIMEOUT_MS;
    const { timeoutMs = defaultTimeoutMs, priority } = callOptions(options);
    return rpc.call(method, params, { timeoutMs, priority, carrier: carrierSwitch.wireFor(method) });
  };

  return {
    deviceId,
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
    /** Ride this DataChannel, or `null` when it has gone. Nothing carries this
     *  session in between. */
    peer: (peerCarrier) => carrierSwitch.peer(peerCarrier),

    /**
     * Put this session's signaling back on the rendezvous, which the caller
     * has reopened to ask for an ICE restart (rule 4).
     *
     * The same id and key are presented, so the bridge takes it as a carrier
     * re-attach rather than a second session, and whatever `rtc.*` was queued
     * while there was no rendezvous is answered with the new wire.
     */
    reattachSignaling: async () => {
      await rendezvous.mint({ sessionId: minted.sessionId, sessionKeyB64: minted.sessionKeyB64 });
      takeSignalingWire(minted);
    },

    /**
     * This device cannot be reached: refuse everything that was waiting for a
     * wire, in the caller's own words (rule 3's blocked reason). The caller is
     * the one telling us, so nothing is reported back to it.
     */
    fail: (error) => {
      severed = true;
      carrierSwitch.fail(error);
      rpc.fail(error);
    },
    /** What re-establishes this session on a carrier it has just taken —
     *  `session.hello` and a read of every mounted surface. Nothing is
     *  dispatched to the user's surfaces before the channels are open (rule 2),
     *  so the first one to run is the first time this session is live. */
    onCarrier: (fn) => (onCarrierChange = fn),
    /** Sever this session deliberately — the device was let go of, or a newer
     *  session for it landed and this one lost the race — with no onLost. */
    close: () => {
      carrierSwitch.close();
      severed = true;
      rpc.close(new Error("session closed"));
      signaling?.close(); // the rendezvous is the caller's; this lease on it is ours
    },
  };
}

/**
 * A session over a relay rendezvous of its own.
 *
 * The adapter the connect sequence is still written against: it opens one
 * rendezvous, mints one session on it and ties the two lifetimes together.
 * Stage 06 replaces it — `connectDevice` owns its device's rendezvous, closes
 * it once the channels are open and reopens it for an ICE restart, and the
 * terminal session is minted on the same one. The options this ignores
 * (`waitForDevice`, `deviceWaitMs`, `onDeviceKey`, `onDeviceOffline`) are the
 * relay presence the api owns now (rule 6).
 */
export async function openRelaySession({
  relayUrl,
  transport,
  WebSocketImpl,
  fetchToken,
  getPinnedDeviceKey,
  preferDeviceId = null,
  acceptTimeoutMs,
  isPaused,
  onLost,
  onPush,
}) {
  const rendezvous = createRelayRendezvous({
    deviceId: preferDeviceId,
    relayUrl,
    transport,
    WebSocketImpl,
    fetchToken,
    getPinnedDeviceKey,
    acceptTimeoutMs,
  });
  let session;
  try {
    session = await openSession({ rendezvous, transport, deviceId: preferDeviceId, isPaused, onLost, onPush });
  } catch (error) {
    rendezvous.close(); // a handshake the caller is told about leaves no socket behind
    throw error;
  }
  return { ...session, close: () => {
    session.close();
    rendezvous.close();
  } };
}
