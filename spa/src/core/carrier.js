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
export function openCarrier({ socket, channel, sessionId }) {
  return channel ? channelCarrier(channel) : relayCarrier(socket, sessionId);
}

/** What every carrier shares: one envelope sink, one close report that fires
 *  at most once, however the wire ended.
 *
 *  Both slots are subscriptions, not settings: a carrier is held by more than
 *  one owner at a time — the session riding it and the link that opened it —
 *  and neither may silently unregister the other. Each returns the
 *  unsubscribe that is the only way off. */
function carrierCore() {
  const envelopeListeners = new Set();
  const closeListeners = new Set();
  let ended = false;
  const subscribe = (listeners) => (fn) => {
    listeners.add(fn);
    return () => listeners.delete(fn);
  };
  return {
    deliver: (envelope) => {
      for (const listener of [...envelopeListeners]) listener(envelope);
    },
    end: () => {
      if (ended) return false;
      ended = true;
      for (const listener of [...closeListeners]) listener();
      return true;
    },
    gone: () => ended,
    interface: {
      onEnvelope: subscribe(envelopeListeners),
      onClose: subscribe(closeListeners),
    },
  };
}

function relayCarrier(socket, sessionId) {
  const core = carrierCore();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(typeof event.data === "string" ? event.data : event.data.toString());
    if (message.type === "e2ee_envelope") core.deliver(message.envelope);
  });
  socket.addEventListener("close", core.end);
  return {
    ...core.interface,
    send: (envelope) => socket.send(JSON.stringify({ type: "e2ee_envelope", session_id: sessionId, envelope })),
    close: () => {
      core.end();
      socket.close();
    },
  };
}

function channelCarrier(channel) {
  const core = carrierCore();
  const reassembler = createReassembler();
  const drainWaiters = [];

  const shutDown = () => {
    if (!core.end()) return;
    for (const { reject } of drainWaiters.splice(0)) reject(new Error("the channel closed"));
    if (channel.readyState === "open" || channel.readyState === "connecting") channel.close();
  };

  channel.addEventListener("close", shutDown);
  channel.addEventListener("error", shutDown);
  channel.addEventListener("message", (event) => {
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
    for (const { resolve } of drainWaiters.splice(0)) resolve();
  });

  const drained = () =>
    new Promise((resolve, reject) => {
      if (core.gone() || channel.readyState !== "open") return reject(new Error("the channel closed"));
      if (channel.bufferedAmount <= DC_BUFFERED_HIGH) return resolve();
      drainWaiters.push({ resolve, reject });
    });

  const write = async (envelope) => {
    for (const part of splitEnvelope(JSON.stringify(envelope))) {
      await drained();
      if (channel.readyState !== "open") throw new Error("the channel closed");
      channel.send(part);
    }
  };

  // One envelope's parts cross without another's between them, and a writer
  // parked at the buffer limit holds the ones behind it rather than reordering.
  let writing = Promise.resolve();
  return {
    ...core.interface,
    send: (envelope) => {
      const written = writing.then(() => write(envelope));
      writing = written.catch(() => {});
      return written;
    },
    close: shutDown,
  };
}
