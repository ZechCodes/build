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

import { createSessionRpc, DEFAULT_RPC_TIMEOUT_MS } from "./sessionRpc.js";
import { createSessionSwitch, isSignaling } from "./sessionSwitch.js";
import { createPathProbe, PATH_PROBE_EVENT, PING_TIMEOUT_MS } from "./pathProbe.js";
import { peerHeardAt } from "./pathLiveness.js";
import { recordConnectionDiagnostic } from "./connectionDiagnostics.js";

export { DEFAULT_RPC_TIMEOUT_MS };

/** How long a restarted path has to show it carries this session (#123).
 *
 *  Longer than the probe's ping on purpose: a restart on a healthy path comes
 *  back to whatever the bridge queued while it was down, and the pong waits
 *  behind it — but any frame at all ends the wait. A path that carries nothing
 *  for this long after its restart says `connected` is not this session's. */
export const CARRY_CONFIRM_MS = 10000;

/** Whether this rejection is the PATH's deadline: a frame that went out and was
 *  never acknowledged, which is the one failure that is about the wire rather
 *  than about the bridge. The answer deadline (a device that has the request and
 *  is taking its time) is deliberately not this, and neither is a refusal — a
 *  probe fired on either would tear a session down over a long-running job.
 *
 *  `deadline` is absent on errors from a bridge adapter that re-wrapped the
 *  rejection, so a timeout that reached here without one is read as the path's:
 *  before the field existed that was the only deadline a call had. */
const rpcHitThePathDeadline = (error) =>
  Boolean(error && error.timedOut && (error.deadline === undefined || error.deadline === "path"));

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
  /** Asks the wire whether it is there (core/pathProbe.js). Stood up below,
   *  because it names the session the rendezvous has not minted yet; declared
   *  here so the teardowns above can reach it. */
  let pathProbe = null;
  /** Whether this session's peer is renegotiating (`watchRecovery`). Nothing
   *  says so until the caller has opened a peer link and handed it over, and a
   *  session with no link is a session nothing is putting right. */
  let peerIsRecovering = () => false;

  /** Nothing is carrying this session any more. The caller hears it once. */
  const severSession = () => {
    if (severed) return;
    severed = true;
    pathProbe?.stop();
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
   * Is the wire still there? Asked when a call burned its whole path deadline.
   *
   * The probe rides the peer carrier this session is on — never the rendezvous,
   * which is not a data plane and would answer for the wrong wire — and it goes
   * through `rpc.call` directly rather than through `rawCall`, so a probe cannot
   * probe itself and an offline pause cannot hold back the question that decides
   * whether the pause should end.
   *
   * A dead verdict severs the session on the spot: that is the down edge the
   * recovery supervisor mints the next one from, the same edge a lost channel
   * produces, so nothing downstream has to learn a new way for a session to end.
   */
  const diagnosticId = `${deviceId}:${minted.sessionId}`;
  pathProbe = createPathProbe({
    ping: () => rpc.call("ping", {}, { timeoutMs: PING_TIMEOUT_MS, carrier: carrierSwitch.active() }),
    wire: () => carrierSwitch.active(),
    rpc: { lastFrameAt: () => rpc?.lastFrameAt() || 0 },
    busy: () => peerIsRecovering() === true,
    diagnosticId,
    onDead: () => {
      // Recorded beside the verdict, because a verdict nobody acted on and a
      // session that was actually restarted read the same in a history
      // otherwise, and this line is the seam between the two: everything after
      // it in the timeline belongs to the next session.
      recordConnectionDiagnostic(diagnosticId, PATH_PROBE_EVENT, { state: "restarting" });
      severSession();
    },
  });

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
    const pending = rpc.call(method, params, { timeoutMs, priority, carrier: carrierSwitch.wireFor(method) });
    // Signaling rides the rendezvous, so its deadline says nothing about the
    // peer path and must not be allowed to judge it.
    if (isSignaling(method)) return pending;
    return pending.catch((error) => {
      // The call fails now, exactly as it always has — the composer still says
      // "Delivery uncertain", and #30 is about what happens NEXT. The probe runs
      // beside that rejection rather than delaying it: a caller waiting three
      // more seconds to be told what it already knows is a worse surface.
      if (rpcHitThePathDeadline(error)) pathProbe?.judge(method);
      throw error;
    });
  };

  return {
    deviceId,
    sessionId: minted.sessionId,
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
     * Ask this session whether its path is still there, deliberately.
     *
     * The same probe an RPC deadline arms (core/pathProbe.js), reachable by name
     * so another channel's verdict can be checked against the app side rather
     * than believed about a wire it cannot see. `asked` names the caller in the
     * diagnostic, because "who wanted to know" is the first thing a reader of the
     * record needs.
     *
     * Answers `"alive"`, `"dead"` or `"no-wire"`. A dead verdict has already
     * severed the session by the time this resolves — the probe owns that — so a
     * caller acts on the answer only to record it.
     */
    probePath: (asked = "asked") => pathProbe?.judge(asked) ?? Promise.resolve("no-wire"),

    /**
     * Tell this session when its peer is renegotiating (core/peerLink.js's
     * recovery status).
     *
     * An ICE restart keeps the channels open while it works, so a call can burn
     * its deadline under one on a path that is about to be perfectly fine — and
     * the restart already has its own deadline and its own teardown. The probe
     * stands down for it rather than racing it to a verdict, exactly as the
     * terminals' probe does.
     */
    watchRecovery: (isRecovering) => {
      peerIsRecovering = typeof isRecovering === "function" ? isRecovering : () => false;
    },

    /**
     * Does the channel this session rides carry it? Asked after an ICE restart
     * reports `connected` (#123), because that word is ICE's and DTLS's: a
     * bridge that restarted answers the restart's offer as a fresh session, and
     * the browser's channels still read `open` from the association the old
     * process took with it. Nothing crosses them.
     *
     * Asks again each time a ping goes unanswered: a path that has just come
     * back can lose the first question into an association still being made.
     * `true` once a ping is answered or any frame, or part of one, arrives
     * after asking; `false` when neither happens inside `timeoutMs`, when the
     * channel refuses the question outright, or when there is no channel. Severs
     * nothing and never rejects: what a dead restart costs is the link's call,
     * and the path probe's own verdict stays latched for real deadlines.
     */
    confirmCarried: async (timeoutMs = CARRY_CONFIRM_MS) => {
      const riding = carrierSwitch.active();
      const asked = Date.now();
      const left = () => timeoutMs - (Date.now() - asked);
      while (riding && !severed && left() > 0) {
        try {
          await rpc.call("ping", {}, { timeoutMs: Math.min(PING_TIMEOUT_MS, left()), carrier: riding });
          return true;
        } catch (error) {
          // Any part counts, not only a whole frame: a restart that reaches
          // the same bridge resumes what was in flight, and a large answer
          // holds the pong behind it on the ordered channel. A part arriving
          // after asking can only come from a process that reaches us.
          if (peerHeardAt(rpc, riding) > asked) return true;
          if (!error?.timedOut) return false;
        }
      }
      return false;
    },

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
      pathProbe?.stop();
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
      pathProbe?.stop();
      rpc.close(new Error("session closed"));
      signaling?.close(); // the rendezvous is the caller's; this lease on it is ours
    },
  };
}
