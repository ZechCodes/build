// A carrier is a wire, not a protocol.
//
// A session's key, its frames, its dispatch and its teardown live above this
// line; the relay socket and a DataChannel are two implementations of one
// narrow interface below it, and nothing above learns which one is carrying.
//
//   { send(envelope), onEnvelope(fn), onClose(fn), close() }
//
// Every carrier in the SPA is built by openCarrier, and the test that picks the
// implementation lives in it and nowhere else.

import { ChunkError, createReassembler, splitEnvelope } from "./chunk.js";

/** How many bytes a channel may hold undelivered before its writer waits (spec
 *  §Backpressure). The limit is per channel, so a terminal flood cannot stall
 *  an RPC reply. Its mirror is DC_BUFFERED_HIGH in bridge/src/rtc.rs. */
export const DC_BUFFERED_HIGH = 1024 * 1024;

/**
 * One wire for one session's envelopes.
 *
 * `{ socket, sessionId }` wraps an authenticated relay socket, whose wire
 * wrapper is `{"type":"e2ee_envelope","session_id":…,"envelope":…}`.
 * `{ channel }` wraps a negotiated DataChannel, whose envelope JSON crosses
 * bare, chunked past the message limit.
 */
export function openCarrier({ socket, channel, sessionId, frames = peerFrames() }) {
  return channel ? channelCarrier(channel, frames) : relayCarrier(socket, sessionId, frames);
}

/** Install a role-bound application codec only after peer proof succeeds.
 * Sequence allocation and handoff share one queue, so canceled calls cannot
 * leave gaps behind an earlier send that is waiting for channel capacity. */
export function bindCarrier(raw, codec) {
  const frameListeners = new Set();
  const closeListeners = new Set();
  let ended = false;
  const lease = new AbortController();
  let stopFrames = () => {};
  let stopClose = () => {};
  const retire = (reason = null) => {
    if (ended) return false;
    ended = true;
    lease.abort(reason || new Error("the bound channel closed"));
    stopFrames();
    stopClose();
    codec.close?.();
    frameListeners.clear();
    for (const listener of [...closeListeners]) listener(reason);
    closeListeners.clear();
    return true;
  };
  const close = (reason = null) => {
    if (retire(reason)) raw.close(reason);
  };
  stopClose = raw.onClose(retire);
  stopFrames = raw.onEnvelope((envelope) => {
    if (ended) return;
    let frame;
    try {
      frame = codec.decode(envelope);
    } catch (error) {
      close(error);
      return;
    }
    // A close is transport control. Its payload can never answer a request
    // or publish a push through the application listeners.
    if (frame.frame_type === "close") {
      close();
      return;
    }
    for (const listener of [...frameListeners]) {
      if (ended) break;
      listener(frame);
    }
  });
  const send = async (payload, { frameType = "data", assertCurrent = () => {}, onHandoff = () => {}, signal } = {}) => {
    const linked = linkedAbort(lease.signal, signal);
    let encoded = false;
    try {
      await raw.sendGenerated(() => {
        encoded = true;
        return codec.encode(payload, frameType);
      }, { signal: linked.signal, assertCurrent, onHandoff });
      if (frameType === "close") close();
    } catch (error) {
      // The physical writer owns partial-frame failure, including retirement
      // after this wrapper ended. A zero-byte cancellation leaves raw reusable.
      if (encoded) {
        if (linked.signal.aborted) retire(error);
        else close(error);
      }
      throw error;
    } finally {
      linked.dispose();
    }
  };
  const subscribe = (listeners) => (fn) => {
    if (!ended) listeners.add(fn);
    return () => listeners.delete(fn);
  };
  return {
    onFrame: subscribe(frameListeners),
    onClose: subscribe(closeListeners),
    // Rebinding a role can reuse its raw channel. Retire only this codec and
    // delivery binding; a stale owner must not close the replacement wire.
    retire,
    close,
    isClosed: () => ended,
    peerFrameAt: () => raw.peerFrameAt(),
    peerPartAt: () => raw.peerPartAt(),
    peerIsConnected: () => raw.peerIsConnected(),
    sendFrame: send,
  };
}

const abortReason = (signal) => signal.reason || new DOMException("The write was cancelled", "AbortError");

function abortable(promise, signal) {
  if (!signal) return promise;
  return new Promise((resolve, reject) => {
    const stop = () => signal.removeEventListener("abort", abort);
    const abort = () => { stop(); reject(abortReason(signal)); };
    signal.addEventListener("abort", abort, { once: true });
    promise.then((value) => { stop(); resolve(value); }, (error) => { stop(); reject(error); });
    if (signal.aborted) abort();
  });
}

function linkedAbort(...signals) {
  const controller = new AbortController();
  const stops = [];
  for (const signal of signals.filter(Boolean)) {
    const abort = () => controller.abort(abortReason(signal));
    signal.addEventListener("abort", abort, { once: true });
    stops.push(() => signal.removeEventListener("abort", abort));
    if (signal.aborted) abort();
  }
  return { signal: controller.signal, dispose: () => stops.forEach((stop) => stop()) };
}

/**
 * What a set of wires knows about the path under them — one record, shared by
 * every carrier that belongs to the same peer.
 *
 * `at` is when a frame last arrived on any of them: a peer link's two channels
 * are one path, and a frame on either is proof that path is up, whichever
 * session it belonged to. Without this each session could only vouch for its
 * own channel, and a terminal session sitting quiet beside a busy app session
 * had no way to tell "nothing is arriving" from "not for me".
 *
 * `connected` is what the browser's own ICE says about the path (`peerLink.js`
 * writes it). It is the stronger evidence of the two and the reason the first
 * is not enough: an idle connection carries no frames at all, and a session
 * that read silence as death tore down a path ICE was still holding open with
 * its own consent checks.
 */
export function peerFrames() {
  return { at: 0, partAt: 0, connected: false };
}

/** `WebSocket.OPEN`, as a number rather than as a global: this module is read
 *  in environments that have no `WebSocket` of their own. */
const SOCKET_OPEN = 1;

/**
 * Write to a relay socket, or say it is gone.
 *
 * Every frame this app puts on a relay socket is written across awaits — a
 * frame's encryption, the transport's session init, a ping the liveness watcher
 * issued two seconds ago — and a bridge reconnect takes the socket out from
 * under any of them. The write that resumes can never arrive, and a browser
 * logs it as "WebSocket is already in CLOSING or CLOSED state". CLOSING is part
 * of the question: a socket someone has called `close()` on stops taking writes
 * at once and reports its close a turn later.
 *
 * The refusal is an ordinary loss, in the words a lost channel is refused in,
 * so the call that frame belonged to fails now rather than waiting out a reply
 * nobody will send.
 */
export function sendOverSocket(socket, text) {
  if (!socket || socket.readyState !== SOCKET_OPEN) throw new Error("the relay socket closed");
  socket.send(text);
}

/** What every carrier shares: one envelope sink, one close report that fires
 *  at most once, however the wire ended.
 *
 *  Both slots are subscriptions, not settings: a carrier is held by more than
 *  one owner at a time — the session riding it and the link that opened it —
 *  and neither may silently unregister the other. Each returns the
 *  unsubscribe that is the only way off. */
function carrierCore(frames) {
  const envelopeListeners = new Set();
  const closeListeners = new Set();
  let ended = false;
  const subscribe = (listeners) => (fn) => {
    listeners.add(fn);
    return () => listeners.delete(fn);
  };
  return {
    deliver: (envelope) => {
      frames.at = Date.now();
      frames.partAt = frames.at;
      for (const listener of [...envelopeListeners]) listener(envelope);
    },
    /** `reason` travels to the close listeners: a wire that was shut on
     *  purpose says why, and the owner of the link decides what that costs. */
    end: (reason = null) => {
      if (ended) return false;
      ended = true;
      for (const listener of [...closeListeners]) listener(reason);
      return true;
    },
    gone: () => ended,
    interface: {
      isClosed: () => ended,
      onEnvelope: subscribe(envelopeListeners),
      onClose: subscribe(closeListeners),
      /** When a frame last arrived on any wire of this peer. */
      peerFrameAt: () => frames.at,
      /** When any part of a frame last arrived: a large envelope crosses as
       *  many, and a path still carrying one has not completed a frame yet. */
      peerPartAt: () => frames.partAt || 0,
      /** Whether the browser's own ICE still holds this path open. */
      peerIsConnected: () => frames.connected === true,
    },
  };
}

/**
 * One session's lease on a relay socket.
 *
 * The socket belongs to the rendezvous that opened it, and one rendezvous mints
 * several sessions on it (spec rule 5), so this carrier is neither the only
 * reader of the wire nor allowed to end it: it takes the frames that name its
 * session and, when it is let go, stops reading and leaves the socket to the
 * others. Only the rendezvous closes the socket.
 */
function relayCarrier(socket, sessionId, frames) {
  const core = carrierCore(frames);
  socket.addEventListener("message", (event) => {
    if (core.gone()) return;
    const message = JSON.parse(typeof event.data === "string" ? event.data : event.data.toString());
    if (message.type !== "e2ee_envelope") return;
    // The relay names the session on every envelope it forwards; an unnamed one
    // is nobody else's, so it goes to whoever is reading.
    if (message.session_id && message.session_id !== sessionId) return;
    core.deliver(message.envelope);
  });
  socket.addEventListener("close", core.end);
  return {
    ...core.interface,
    send: (envelope) => sendOverSocket(socket, JSON.stringify({ type: "e2ee_envelope", session_id: sessionId, envelope })),
    close: (reason = null) => {
      core.end(reason);
    },
  };
}

function channelCarrier(channel, frames) {
  const core = carrierCore(frames);
  const reassembler = createReassembler();
  const drainWaiters = new Set();

  const shutDown = (reason = null) => {
    if (!core.end(reason)) return;
    for (const { reject } of drainWaiters) reject(new Error("the channel closed"));
    drainWaiters.clear();
    if (channel.readyState === "open" || channel.readyState === "connecting") channel.close();
  };

  channel.addEventListener("close", () => shutDown());
  channel.addEventListener("error", () => shutDown());
  channel.addEventListener("message", (event) => {
    // Before reassembly: a part is proof this path reaches the peer even while
    // the envelope it belongs to is still arriving (#123).
    frames.partAt = Date.now();
    const text = typeof event.data === "string" ? event.data : new TextDecoder().decode(event.data);
    let envelope;
    try {
      const envelopeJson = reassembler.accept(text);
      if (envelopeJson === null) return;
      envelope = JSON.parse(envelopeJson);
    } catch (error) {
      // The two things a peer can say that this channel cannot come back from:
      // parts that do not add up, and parts that add up to something that is
      // not an envelope. Neither can be resumed — parts carry no way to ask for
      // one again — so the channel goes and the session falls back to whatever
      // else is carrying it. Anything else thrown here is a bug in the
      // reassembler and belongs to whoever reads the stack.
      if (!(error instanceof ChunkError) && !(error instanceof SyntaxError)) throw error;
      shutDown();
      return;
    }
    core.deliver(envelope);
  });

  channel.bufferedAmountLowThreshold = DC_BUFFERED_HIGH;
  channel.addEventListener("bufferedamountlow", () => {
    for (const { resolve } of drainWaiters) resolve();
    drainWaiters.clear();
  });

  const drained = (signal) => {
    let waiter;
    const ready = new Promise((resolve, reject) => {
      if (core.gone() || channel.readyState !== "open") return reject(new Error("the channel closed"));
      if (channel.bufferedAmount <= DC_BUFFERED_HIGH) return resolve();
      waiter = { resolve, reject };
      drainWaiters.add(waiter);
    });
    return abortable(ready, signal).finally(() => drainWaiters.delete(waiter));
  };

  const write = async (envelope) => {
    for (const part of splitEnvelope(JSON.stringify(envelope))) {
      await drained();
      if (channel.readyState !== "open") throw new Error("the channel closed");
      channel.send(part);
    }
  };

  // A generated application write allocates its sequence only after initial
  // capacity is available on the actual physical queue. Each await and caller
  // callback is followed by an allocation check before another physical byte.
  const writeGenerated = async (prepare, { signal, assertCurrent = () => {}, onHandoff = () => {} }) => {
    let attempted = false;
    const leaseCurrent = () => {
      if (signal?.aborted) throw abortReason(signal);
      if (core.gone() || channel.readyState !== "open") throw new Error("the channel closed");
    };
    const current = () => {
      leaseCurrent();
      assertCurrent();
      leaseCurrent(); // Caller assertions can synchronously retire this lease.
    };
    const abort = () => { if (attempted) shutDown(abortReason(signal)); };
    signal?.addEventListener("abort", abort, { once: true });
    try {
      await drained(signal);
      current();
      const parts = splitEnvelope(JSON.stringify(prepare()));
      current();
      for (const part of parts) {
        if (attempted) { await drained(signal); current(); }
        if (!attempted) { onHandoff(); current(); }
        attempted = true;
        channel.send(part);
        current();
      }
    } catch (error) {
      if (attempted) shutDown(error);
      throw error;
    } finally {
      signal?.removeEventListener("abort", abort);
    }
  };

  // Legacy controls and generated writes share one physical queue.
  let writing = Promise.resolve();
  const enqueue = (operation) => {
    const written = writing.then(operation);
    writing = written.catch(() => {});
    return written;
  };
  return {
    ...core.interface,
    send: (envelope) => enqueue(() => write(envelope)),
    sendGenerated: (prepare, options = {}) => abortable(enqueue(() => writeGenerated(prepare, options)), options.signal),
    close: shutDown,
  };
}
