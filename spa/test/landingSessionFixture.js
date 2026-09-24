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
    /** Answer the latest request for `method`: a re-greeting on the same
     *  carrier sends a second session.hello. */
    answer(method, result) {
      const asked = this.sent.findLast((request) => request.method === method);
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
export async function landSession(deviceId, context, modules) {
  return attachSession(await openLandingSession(deviceId, modules), context, modules);
}

let minted = 0;

/** A session for `deviceId`, opened and not yet adopted: the first half of
 *  landSession, for a suite that has to choose the moment it lands. */
export async function openLandingSession(deviceId, { openSession }) {
  const signal = carrier();
  const session = await openSession({
    deviceId,
    transport,
    rendezvous: {
      mint: async () => ({ sessionId: `s${++minted}`, deviceId, sessionKeyB64: "key" }),
      signalCarrier: () => signal,
    },
  });
  return { session, peer: carrier() };
}

/** And the second half: adopt it, arm its greeting on the carrier, attach the
 *  peer. `lifetime` is the adoption's, for a suite that loses the connection. */
export function attachSession({ session, peer }, context, { adoptDeviceConnection, greetLiveBridge }) {
  const { lifetime } = adoptDeviceConnection(session);
  session.onCarrier(() => greetLiveBridge(context));
  const landed = session.peer(peer);
  return { session, peer, landed, lifetime };
}
