// One E2EE session's crypto and correlation, over one carrier at a time.
//
// The key, the frames, the pending calls and the demux live here, above the
// wire; which wire is carrying is one call, `rideOn`, and nothing else in the
// module asks. That is "one session, two carriers" as one operation: the app
// session and the terminal session are the same machinery over different
// payloads, and this is the machinery.
//
// Hides the pending map, the request ids, encrypt/decrypt, the per-call
// timeout, and the rule that tells a reply from a push.

/** How long a call waits for its answer before it is not coming.
 *
 *  This is the PATH's deadline: how long a frame may go unacknowledged before
 *  the wire under it is the thing in doubt. Past a receipt it no longer
 *  applies — see [`ANSWER_TIMEOUT_MS`]. */
export const DEFAULT_RPC_TIMEOUT_MS = 12000;

/** How long a call waits once the bridge has said it has the request.
 *
 *  A receipt separates two questions a single deadline was answering at once:
 *  did this reach the device, and is the device taking too long. The first is
 *  about the wire and is worth giving up on; the second is a machine with
 *  eleven agents on it doing what it was asked, and giving up there is how a
 *  client reported a message it had safely delivered as failed. So the wait
 *  after a receipt is long enough to be a real fault rather than a queue. */
export const ANSWER_TIMEOUT_MS = 120000;

/**
 * One call's two deadlines: the path's, and the answer's.
 *
 * Until a receipt arrives the call is waiting on the wire, and `timeoutMs` is
 * what says the wire is not carrying. A receipt is the device saying it has
 * the request, and from then on the only question is how long the work takes —
 * so the path's deadline is dropped and a far longer one takes its place,
 * because a queue is not a fault and reporting it as one is how a delivered
 * message read as failed.
 */
function createDeadline(method, timeoutMs, onReceipt) {
  let timer = null;
  let settle = null;
  let receipted = false;
  const arm = (waitMs, uncertain, which) => {
    clearTimeout(timer);
    if (waitMs == null || waitMs === 0) return;
    timer = setTimeout(() => settle?.(timedOutError(method, uncertain(), which)), waitMs);
  };
  return {
    race(answer, uncertain) {
      const expiry = new Promise((_, reject) => {
        settle = reject;
        arm(timeoutMs, uncertain, "path");
      });
      return Promise.race([answer, expiry]);
    },
    receipted(handoffAttempted) {
      if (receipted) return;
      receipted = true;
      arm(ANSWER_TIMEOUT_MS, () => handoffAttempted, "answer");
      onReceipt?.();
    },
    done() {
      clearTimeout(timer);
    },
  };
}

/** `deadline` says WHICH of the two fired, because they mean opposite things
 *  about the wire: `"path"` is a frame that went unacknowledged and is reason to
 *  doubt the connection (what core/pathProbe.js acts on), while `"answer"` is a
 *  device that has the request and is taking its time, which says the wire is
 *  fine. A single `timedOut` flag could not tell them apart, and a probe that
 *  fired on the second would tear down a session over a long-running job. */
function timedOutError(method, uncertain = false, deadline = "path") {
  const error = new Error(`${method} timed out`);
  error.timedOut = true;
  error.uncertain = uncertain;
  error.deadline = deadline;
  return error;
}

/** The bridge said no. From API 1.1 the refusal carries `error_code`,
 *  `retryable` and `details` beside the string (wire spec step 2.4); they ride
 *  the Error under their wire names so the adapter can read them off it. A 1.0
 *  bridge sends the string alone, and the Error carries nothing more. */
function refusalError(payload) {
  const error = new Error(payload.error);
  for (const field of ["error_code", "retryable", "details"]) {
    if (payload[field] !== undefined) error[field] = payload[field];
  }
  return error;
}

/** A carrier disappearing after send was invoked cannot tell us whether the
 * device accepted the request. Give each affected call its own error: calls
 * that were still waiting for a carrier can fail definitely beside them. */
function uncertainDeliveryError(reason) {
  const error = new Error((reason && reason.message) || String(reason));
  if (reason && reason.name) error.name = reason.name;
  if (reason && typeof reason === "object") Object.assign(error, reason);
  error.uncertain = true;
  return error;
}

/**
 * @param transport the injected crypto layer (`@build/secure-transport`).
 * @param sessionId, sessionKeyB64, deviceId what the rendezvous minted:
 *   this session's identity, key, and the device it was sealed to. They outlive
 *   every carrier.
 * @param noCarrier what a call fails with when nothing is carrying. Its owner
 *   chooses the words, because what a caller does about it differs: the app
 *   goes offline, a terminal surface waits the socket out.
 */
export function createSessionRpc({
  transport,
  sessionId,
  sessionKeyB64,
  deviceId,
  noCarrier = () => new Error("nothing is carrying this session"),
  timeoutMs: defaultTimeoutMs = DEFAULT_RPC_TIMEOUT_MS,
}) {
  const pending = new Map();
  const pushListeners = new Set();
  let carrier = null;
  let requestId = 0;
  let lastFrameAt = 0;
  let closed = false;

  /** One decrypted frame off whichever carrier brought it: somebody's answer,
   *  or the bridge saying something moved. A frame written for another session
   *  is one this key cannot open, and is not this session's to read. */
  const takeEnvelope = async (envelope) => {
    let frame;
    try {
      frame = await transport.decryptEnvelope({ sessionKeyB64, envelope });
    } catch {
      return;
    }
    lastFrameAt = Date.now(); // whatever it says, the bridge reached us
    const payload = frame.payload;
    const answered = payload && payload.id !== undefined ? pending.get(payload.id) : null;
    if (answered) {
      // A receipt settles nothing: it says the device has the request, which
      // takes the call off the path's deadline and onto the answer's. It
      // carries no `ok`, which is what tells the two apart.
      if (payload.accepted === true && payload.ok === undefined) {
        answered.receipt();
        return;
      }
      pending.delete(payload.id);
      answered.ok(payload);
      return;
    }
    // Nobody asked for this: a frame with a `type` and no request behind it is
    // a push; anything else is a reply to a call that already gave up, and has
    // nowhere left to go.
    if (payload && payload.type) for (const listener of [...pushListeners]) listener(payload);
  };

  /** Every call still waiting, told why it will never answer. */
  const fail = (error) => {
    for (const waiting of pending.values()) {
      waiting.reject(waiting.handoffAttempted ? uncertainDeliveryError(error) : error);
    }
    pending.clear();
  };

  return {
    sessionId,
    deviceId,

    /** Read this carrier's frames. Every carrier the session holds is read
     *  from, not only the one it sends on: signaling is pinned to the relay, so
     *  its answers arrive on a wire this session may not be sending over.
     *  Registering the same reader twice is registering it once. */
    readFrom(held) {
      held?.onEnvelope(takeEnvelope);
    },

    /** The wire this session sends on now, `null` when nothing is carrying. */
    rideOn(taken) {
      carrier = taken || null;
      lastFrameAt = 0; // the wire that just went vouches for nothing here
      taken?.onEnvelope(takeEnvelope);
    },

    /** Subscribe to what the bridge says without being asked. Returns the
     *  unsubscribe, which is the only way off. */
    onPush(fn) {
      pushListeners.add(fn);
      return () => pushListeners.delete(fn);
    },

    /** When this session last read a frame on the wire it is riding. Any frame
     *  is itself proof the bridge is reachable, which is what lets a liveness
     *  probe skip a busy stream. */
    lastFrameAt: () => lastFrameAt,

    /**
     * One encrypted request, and the reply it is waiting for.
     *
     * `carrier` pins the call to one wire — how signaling stays on the relay
     * while a channel carries everything else. It may be the *promise* of a
     * wire that is on its way back, and then the wait is inside this call's
     * own deadline: nothing waits on a carrier longer than it would have
     * waited for an answer over one.
     *
     * `priority: "background"` rides the request envelope beside `id` and
     * `method` (wire spec step 1.4), so the bridge's dispatcher keeps a cache
     * warm-up out of the focused surface's way. Anything else is foreground
     * and stamps nothing: absence is the default on both ends.
     * A `null` or zero `timeoutMs` leaves the call pending until an answer or
     * session failure; it does not create a browser timer.
     */
    async call(
      method,
      params = {},
      { timeoutMs = defaultTimeoutMs, carrier: wire = carrier, priority = "foreground", onReceipt = null } = {},
    ) {
      if (closed) throw noCarrier();
      const id = "r" + ++requestId;
      let waiting;
      const deadline = createDeadline(method, timeoutMs, onReceipt);
      const answer = new Promise((resolve, reject) => {
        waiting = {
          handoffAttempted: false,
          reject,
          receipt: () => deadline.receipted(waiting.handoffAttempted),
          ok: (payload) => (payload.ok ? resolve(payload.result) : reject(refusalError(payload))),
        };
        pending.set(id, waiting);
      });
      // A frame that never crossed the wire has no answer coming: the call
      // fails then rather than waiting out a timeout for a reply nobody will
      // send.
      const delivered = (async () => {
        const sending = await wire;
        if (closed || pending.get(id) !== waiting) return;
        if (!sending) throw noCarrier();
        const envelope = await transport.encryptFrame({
          sessionKeyB64,
          outerFields: { session_id: sessionId, route_to: `device:${deviceId}` },
          frameFields: {
            frame_type: "data",
            sender: "client",
            payload: { method, id, params, ...(priority === "background" ? { priority } : {}) },
          },
        });
        // The session may have been failed, closed, or timed out while its
        // carrier/encryption was pending. Once the outward call has settled,
        // its request must never cross later and mutate the old scope.
        if (closed || pending.get(id) !== waiting) return;
        waiting.handoffAttempted = true;
        await sending.send(envelope);
      })();
      delivered.catch((error) => {
        if (pending.get(id) !== waiting) return;
        pending.delete(id);
        waiting.reject(error);
      });
      // However this settles, nothing is waiting for it any more: a call that
      // timed out must not leave an entry for a later loss to reject at nobody.
      const settled = deadline.race(answer, () => waiting.handoffAttempted);
      return settled.finally(() => {
        deadline.done();
        pending.delete(id);
      });
    },

    fail,

    /** The client said so: nothing is asked or answered on this session again.
     *  `reason` is what the calls in flight are told, when the client has a
     *  better answer than "nothing is carrying". */
    close(reason) {
      closed = true;
      carrier = null;
      fail(reason || noCarrier());
    },
  };
}
