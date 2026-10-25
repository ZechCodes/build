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

import { createSqlReadCredits, isSqlRead, SQL_READ_LIMIT } from "./sqlReadCredits.js";
import { createBridgeEpoch, endedOpeningError } from "./bridgeEpoch.js";

/** How long a call waits for its answer before it is not coming.
 *
 *  This is the PATH's deadline: how long a frame may go unacknowledged before
 *  the wire under it is the thing in doubt. Past a receipt it no longer
 *  applies — see [`ANSWER_TIMEOUT_MS`]. */
export const DEFAULT_RPC_TIMEOUT_MS = 12000;

/** A background SQL read may wait this long for sender credit. This is local
 * admission, independent of the caller's path/answer timeout opt-out. */
export const SQL_ADMISSION_TIMEOUT_MS = 12000;

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
 * One call's deadlines: local SQL admission, the path, then the answer.
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
  const expiry = new Promise((_, reject) => { settle = reject; });
  const arm = (waitMs, uncertain, which, onExpire) => {
    clearTimeout(timer);
    if (waitMs == null || waitMs === 0) return;
    timer = setTimeout(() => {
      const error = timedOutError(method, uncertain(), which);
      // Remove an expired admission synchronously, before a late reply can
      // drain the credit queue in this same turn.
      onExpire?.(error);
      settle?.(error);
    }, waitMs);
  };
  return {
    admission(sql, priority, cancel) {
      if (sql && priority === "background") arm(SQL_ADMISSION_TIMEOUT_MS, () => false, "admission", cancel);
    },
    start: (uncertain) => arm(timeoutMs, uncertain, "path"),
    race: (answer) => Promise.race([answer, expiry]),
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

/** `deadline` distinguishes local `"admission"` (definitely unsent) from
 *  the two delivery deadlines, because they mean opposite things
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
  if (deadline === "admission") error.retryable = true;
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

const sqlDeliveryError = (reason) => Object.assign(uncertainDeliveryError(reason), { retryable: true });

function watchCancellation(signal, cancel) {
  if (!signal) return () => {};
  const abort = () => cancel(signal.reason || new DOMException("The request was cancelled", "AbortError"));
  signal.addEventListener("abort", abort, { once: true });
  if (signal.aborted) abort();
  return () => signal.removeEventListener("abort", abort);
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
  sqlReadLimit = SQL_READ_LIMIT, // saturation fixtures inject N=B; production stays at eight
  bridgeEpoch = createBridgeEpoch(),
}) {
  const pending = new Map();
  const sqlReads = createSqlReadCredits(sqlReadLimit);
  const readers = new WeakSet();
  const closedCarriers = new WeakSet();
  const pushListeners = new Set();
  const writeListeners = new Set();
  let carrier = null;
  let requestId = 0;
  let lastFrameAt = 0;
  let closed = false;

  /** One decrypted frame off whichever carrier brought it: somebody's answer,
   *  or the bridge saying something moved. A frame written for another session
   *  is one this key cannot open, and is not this session's to read. */
  const takeEnvelope = async (envelope, source) => {
    let frame;
    try {
      frame = await transport.decryptEnvelope({ sessionKeyB64, envelope });
    } catch {
      return;
    }
    if (closed || !acceptCarrier(frame.payload, source) || !acceptEpoch(frame.payload)) return;
    lastFrameAt = Date.now();
    takePayload(frame.payload, source);
  };

  // A hello is the correlated answer to an explicit authority check. It can
  // report a different epoch, but only connection's current greeting may adopt
  // it. No uncorrelated frame, including an unsolicited hello, changes authority.
  const acceptEpoch = (payload) => {
    const method = pending.get(payload?.id)?.method;
    if (method === "session.hello") return true;
    // The peer needs bridge candidates before its channels can open and carry
    // the first greeting. Session-authenticated ICE pushes establish no epoch.
    if (!bridgeEpoch.current() && (bootstrapMethod(method) || payload?.type === "rtc.ice")) return true;
    return bridgeEpoch.accepts(payload);
  };

  // A subscription belongs to its carrier as well as its session opening.
  // Correlated write replies can finish on their captured carrier after a
  // migration. Uncorrelated outcomes only belong to the current subscription.
  const acceptCarrier = (payload, source) => {
    const waiting = pending.get(payload?.id);
    if (!writeFrame(payload) && !writeCall(waiting)) return true;
    return source === (waiting ? waiting.sendingCarrier : carrier);
  };

  const routeWriteFrame = (payload) => {
    if (payload?.write?.phase === "ack" || payload?.type === "writes.confirmed") {
      for (const listener of [...writeListeners]) listener(payload);
    }
  };

  const takePayload = (payload, source) => {
    if (!acceptSqlReply(payload, source)) return;
    routeWriteFrame(payload);
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

  const acceptSqlReply = (payload, source) => {
    if (!sqlReads.holds(payload?.id)) return true;
    if (!sqlReads.accepts(payload.id, source)) return false;
    if (!(payload.accepted === true && payload.ok === undefined)) sqlReads.finish(payload.id);
    return true;
  };

  const readFrom = (held) => {
    if (!held || readers.has(held)) return;
    readers.add(held);
    held.onEnvelope((envelope) => takeEnvelope(envelope, held));
    held.onClose(() => {
      closedCarriers.add(held);
      sqlReads.releaseCarrier(held, sqlDeliveryError(new Error("the channel closed")));
    });
  };

  const resetSqlReads = (reason = new Error("the SQL read path was restored")) => sqlReads.reset(sqlDeliveryError(reason));

  const prepareCarrier = (sql, id, wire, reject) => {
    if (!sql) return wire;
    const ready = Promise.resolve(wire).then((sending) => {
      sqlReads.bind(id, sending);
      if (closedCarriers.has(sending)) throw sqlDeliveryError(new Error("the channel closed"));
      return sending;
    });
    // Watch even a caller still queued for credit: a failed/closed carrier
    // retires its unsent calls without waiting for another read to finish.
    ready.catch((error) => { reject(error); sqlReads.finish(id); });
    return ready;
  };

  /** Every call still waiting, told why it will never answer. */
  const fail = (error) => {
    resetSqlReads(error);
    for (const waiting of pending.values()) {
      waiting.reject(waiting.handoffAttempted ? uncertainDeliveryError(error) : error);
    }
    pending.clear();
  };

  const stopEpoch = bridgeEpoch.onChange((_current, previous) => {
    if (!previous) return;
    for (const waiting of pending.values()) waiting.reject(endedOpeningError(waiting.method, waiting.handoffAttempted));
    pending.clear();
    resetSqlReads(endedOpeningError("sql", true));
  });

  return {
    sessionId,
    deviceId,
    bridgeEpoch,
    onWriteFrame(fn) {
      writeListeners.add(fn);
      return () => writeListeners.delete(fn);
    },

    /** Read this carrier's frames. Every carrier the session holds is read
     *  from, not only the one it sends on: signaling is pinned to the relay, so
     *  its answers arrive on a wire this session may not be sending over.
     *  Registering the same reader twice is registering it once. */
    readFrom,

    /** The wire this session sends on now, `null` when nothing is carrying. */
    rideOn(taken) {
      carrier = taken || null;
      lastFrameAt = 0; // the wire that just went vouches for nothing here
      readFrom(taken);
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
     * SQL reads start that deadline after sender admission. Background SQL
     * reads have a separate bounded admission wait, even with timeoutMs null/0.
     * An optional
     * `signal` cancels the caller; once sent, its SQL credit stays occupied
     * until the bridge answers or the carrying path is retired.
     */
    async call(
      method,
      params = {},
      { timeoutMs = defaultTimeoutMs, carrier: wire = carrier, priority = "foreground", onReceipt = null, assertCurrent = () => {}, signal, write } = {},
    ) {
      if (closed) throw noCarrier();
      const id = "r" + ++requestId;
      let waiting;
      const deadline = createDeadline(method, timeoutMs, onReceipt);
      const answer = new Promise((resolve, reject) => {
        waiting = {
          method,
          write: Boolean(write),
          sendingCarrier: null,
          handoffAttempted: false,
          reject,
          receipt: () => deadline.receipted(waiting.handoffAttempted),
          ok: (payload) => (payload.ok ? resolve(write ? payload : payload.result) : reject(refusalError(payload))),
        };
        pending.set(id, waiting);
      });
      const reject = (error) => {
        if (pending.get(id) !== waiting) return;
        pending.delete(id);
        waiting.reject(error);
      };
      const sql = isSqlRead(method);
      const wireReady = prepareCarrier(sql, id, wire, reject);
      // Preparation failures are definitely unsent. Once send is invoked,
      // a rejection can follow written DataChannel chunks: reject the caller
      // but retain SQL credit until a reply or carrier/opening release.
      const deliver = async () => {
        const sending = await wireReady;
        if (closed || pending.get(id) !== waiting) return;
        if (!sending) throw noCarrier();
        assertCurrent();
        const envelope = await transport.encryptFrame({
          sessionKeyB64,
          outerFields: { session_id: sessionId, route_to: `device:${deviceId}` },
          frameFields: {
            frame_type: "data",
            sender: "client",
            payload: { method, id, params, ...(priority === "background" ? { priority } : {}), ...(write ? { write: writeMetadata(write) } : {}) },
          },
        });
        // The session may have been failed, closed, or timed out while its
        // carrier/encryption was pending. Once the outward call has settled,
        // its request must never cross later and mutate the old scope.
        if (closed || pending.get(id) !== waiting) return;
        assertCurrent();
        waiting.sendingCarrier = sending;
        waiting.handoffAttempted = true;
        sqlReads.sent(id);
        await sending.send(envelope);
      };
      const start = () => {
        deadline.start(() => waiting.handoffAttempted);
        deliver().catch((error) => {
          reject(sql && waiting.handoffAttempted ? sqlDeliveryError(error) : error);
          sqlReads.cancel(id);
        });
      };
      const settled = deadline.race(answer);
      deadline.admission(sql, priority, (error) => {
        reject(error);
        sqlReads.cancel(id);
      });
      if (sql) sqlReads.reserve(id, start, reject);
      else start();
      const stopCancellation = watchCancellation(signal, (error) => { reject(error); sqlReads.cancel(id); });
      // However this settles, nothing is waiting for it any more: a call that
      // timed out must not leave an entry for a later loss to reject at nobody.
      return settled.finally(() => {
        deadline.done();
        stopCancellation();
        pending.delete(id);
        sqlReads.cancel(id);
      });
    },

    fail,
    resetSqlReads,

    /** The client said so: nothing is asked or answered on this session again.
     *  `reason` is what the calls in flight are told, when the client has a
     *  better answer than "nothing is carrying". */
    close(reason) {
      closed = true;
      stopEpoch();
      carrier = null;
      fail(reason || noCarrier());
    },
  };
}

// The wire carries the durable identity and dependencies beside params. Mode
// selection belongs to the greeting's allowlist; callers cannot override it.
function writeMetadata(write) {
  return { operation_id: write.operation_id, ...(write.after === undefined ? {} : { after: write.after }) };
}

const bootstrapMethod = (method) => method === "ping" || method?.startsWith("rtc.");
const writeFrame = (payload) => payload?.write?.phase === "ack" || payload?.type === "writes.confirmed";
const writeCall = (waiting) => waiting?.write || waiting?.method.startsWith("writes.");
