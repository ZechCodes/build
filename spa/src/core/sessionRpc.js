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

/** How long a call waits for its answer before it is not coming. */
export const DEFAULT_RPC_TIMEOUT_MS = 12000;

function timedOutError(method, uncertain = false) {
  const error = new Error(`${method} timed out`);
  error.timedOut = true;
  error.uncertain = uncertain;
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
 * @param sessionId, sessionKeyB64, deviceId what the relay handshake minted:
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
     */
    async call(
      method,
      params = {},
      { timeoutMs = defaultTimeoutMs, carrier: wire = carrier, priority = "foreground" } = {},
    ) {
      if (closed) throw noCarrier();
      const id = "r" + ++requestId;
      let waiting;
      const answer = new Promise((resolve, reject) => {
        waiting = {
          handoffAttempted: false,
          reject,
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
      return Promise.race([
        answer,
        new Promise((_, reject) =>
          setTimeout(() => reject(timedOutError(method, waiting.handoffAttempted)), timeoutMs),
        ),
      ]).finally(() => pending.delete(id));
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
