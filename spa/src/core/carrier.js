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

import { createReassembler, splitEnvelope } from "./chunk.js";

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
 *  at most once, however the wire ended. */
function carrierCore() {
  let onEnvelope = () => {};
  let onClose = () => {};
  let ended = false;
  return {
    deliver: (envelope) => onEnvelope(envelope),
    end: () => {
      if (ended) return false;
      ended = true;
      onClose();
      return true;
    },
    gone: () => ended,
    interface: {
      onEnvelope: (fn) => (onEnvelope = fn),
      onClose: (fn) => (onClose = fn),
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
    let envelopeJson;
    try {
      envelopeJson = reassembler.accept(text);
    } catch {
      // A reassembly that lost a part cannot be resumed: the channel goes, and
      // the session falls back to whatever else is carrying it.
      shutDown();
      return;
    }
    if (envelopeJson !== null) core.deliver(JSON.parse(envelopeJson));
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
