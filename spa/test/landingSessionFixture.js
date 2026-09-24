// A machine's session landing over the real session, RPC and greeting code,
// with only the wire stood in for: what a suite needs to see which requests a
// landing machine is sent, and in what order, before and after its greeting.

/** An in-memory transport: the envelope carries the frame as it is. */
export const transport = {
  encryptFrame: async ({ outerFields, frameFields }) => ({ outerFields, frameFields }),
  decryptEnvelope: async ({ envelope }) => ({ payload: envelope.frameFields.payload }),
};

/** A carrier that records every request sent over it and answers on cue. */
export function carrier() {
  const readers = new Set();
  return {
    sent: [],
    onClose: () => () => {},
    onEnvelope: (fn) => {
      readers.add(fn);
      return () => readers.delete(fn);
    },
    close() {},
    send(envelope) {
      this.sent.push(envelope.frameFields.payload);
    },
    answer(method, result) {
      const asked = this.sent.find((request) => request.method === method);
      for (const fn of readers) fn({ frameFields: { payload: { id: asked.id, ok: true, result } } });
    },
  };
}

/** What this tab was asked over the carrier, in the order it asked. */
export const asked = (peer) => peer.sent.map((request) => request.method);

/** An API version this tab has an adapter for (core/bridgeApi). */
export const SUPPORTED_API = "1.22.0";

/** An API version nothing here speaks. */
export const UNSUPPORTED_API = "99.0.0";

/**
 * Open `deviceId`'s session and land it the way connection.js lands one:
 * adopted first, its greeting armed on the carrier, then the peer attached.
 * `modules` are the suite's own imports, so a suite that resets its modules
 * lands the session on the registry it is looking at.
 */
export async function landSession(deviceId, context, { openSession, adoptDeviceConnection, greetLiveBridge }) {
  const signal = carrier();
  const peer = carrier();
  const session = await openSession({
    deviceId,
    transport,
    rendezvous: {
      mint: async () => ({ sessionId: "s", deviceId, sessionKeyB64: "key" }),
      signalCarrier: () => signal,
    },
  });
  adoptDeviceConnection(session);
  session.onCarrier(() => greetLiveBridge(context));
  const landed = session.peer(peer);
  return { session, peer, landed };
}
