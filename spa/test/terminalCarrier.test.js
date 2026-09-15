// The terminal socket and the wire it is handed, with no relay anywhere.
//
// A terminal session is minted through the followed device's rendezvous and
// adopted; the `term` DataChannel of that device's peer link is handed over.
// Sealed under that session's key and readable only under it, so a frame that
// outlives the session it was written for is one this socket cannot read —
// exactly as the real transport has it.

import { describe, it, expect, vi } from "vitest";
import { TerminalSocket } from "../src/terminal/session.js";

const fakeTransport = {
  encryptFrame: async ({ sessionKeyB64, outerFields, frameFields }) => ({ key: sessionKeyB64, outerFields, frameFields }),
  decryptEnvelope: async ({ sessionKeyB64, envelope }) => {
    if (envelope.key !== sessionKeyB64) throw new Error("not this session's key");
    return { payload: envelope.frameFields.payload };
  },
};

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = async () => {
  for (let i = 0; i < 8; i++) await tick();
};

const terminalSession = (sessionId, deviceId = "dev-b") => ({
  sessionId,
  sessionKeyB64: `key-${sessionId}`,
  deviceId,
});

/** A `term` channel the test drives, answering every RPC the way the bridge
 *  would so an attach can finish. */
function fakeCarrier({ deafTo = [], sendFails = null } = {}) {
  const envelopeListeners = new Set();
  const closeListeners = new Set();
  const subscribe = (listeners) => (fn) => {
    listeners.add(fn);
    return () => listeners.delete(fn);
  };
  const deliver = (envelope) => envelopeListeners.forEach((fn) => fn(envelope));
  const carrier = {
    sent: [],
    closed: false,
    // The key the bridge on the other end of this channel answers under: the
    // one the frame it is answering was sealed with.
    key: null,
    async send(envelope) {
      if (sendFails) throw new Error(sendFails);
      carrier.sent.push(envelope);
      carrier.key = envelope.key;
      const { method, id } = envelope.frameFields.payload;
      if (deafTo.includes(method)) return;
      const result =
        method === "term.attach" || method === "agent.attach"
          ? { snapshot: "", cursor: 7, term_id: envelope.frameFields.payload.params.term_id }
          : {};
      Promise.resolve().then(() => deliver({ key: envelope.key, frameFields: { payload: { id, ok: true, result } } }));
    },
    onEnvelope: subscribe(envelopeListeners),
    onClose: subscribe(closeListeners),
    close: () => {
      if (carrier.closed) return;
      carrier.closed = true;
      closeListeners.forEach((fn) => fn());
    },
    push: (payload) => deliver({ key: carrier.key, frameFields: { payload } }),
    calls: (method) => carrier.sent.map((e) => e.frameFields.payload).filter((p) => p.method === method),
  };
  return carrier;
}

/** A socket on one session, with the statuses its panes were shown. */
function standing(session = terminalSession("sess-1")) {
  const socket = new TerminalSocket({ transport: fakeTransport });
  const statuses = [];
  socket.onStatus((status) => statuses.push(status));
  socket.adoptTerminalSession(session);
  return { socket, statuses };
}

/** One registered user terminal, its screen collected. */
async function withTerminal(socket, carrier) {
  const screen = { output: [], snapshots: [] };
  const attaching = socket.attachTerminal("term-1", {
    onOutput: (bytes) => screen.output.push(new TextDecoder().decode(bytes)),
    onSnapshot: (bytes) => screen.snapshots.push(new TextDecoder().decode(bytes)),
  });
  await attaching;
  expect(carrier.calls("term.attach")).toHaveLength(1);
  return screen;
}

describe("a terminal socket on the wire it was handed", () => {
  it("re-attaches every open terminal on the wire and sends there from then on", async () => {
    const { socket, statuses } = standing();
    const carrier = fakeCarrier();
    await socket.peer(carrier);
    await withTerminal(socket, carrier);

    const next = fakeCarrier();
    socket.peer(null);
    await socket.peer(next); // the same session, a second channel

    expect(next.calls("term.attach").map((p) => p.params.term_id)).toEqual(["term-1"]);
    await socket.input("term-1", "ls\n");
    expect(next.calls("term.input")).toHaveLength(1);
    expect(carrier.calls("term.input")).toHaveLength(0);
    expect(statuses).toEqual(["connecting", "connected", "disconnected", "connected"]);
    socket.close();
  });

  it("applies the wire's pushes to the screen they name", async () => {
    const { socket } = standing();
    const carrier = fakeCarrier();
    await socket.peer(carrier);
    const screen = await withTerminal(socket, carrier);

    carrier.push({ type: "term.output", term_id: "term-1", cursor: 9, data: btoa("hi") });
    await settle();
    expect(screen.output).toEqual(["hi"]);
    socket.close();
  });

  it("reports the shells lost the moment the wire goes, with nothing to fall back to", async () => {
    const { socket, statuses } = standing();
    const carrier = fakeCarrier();
    carrier.onClose(() => socket.peer(null)); // its owner hands it back
    await socket.peer(carrier);
    await withTerminal(socket, carrier);
    statuses.length = 0;

    carrier.close();
    await tick();

    expect(statuses).toEqual(["disconnected"]);
    await expect(socket.input("term-1", "x")).rejects.toThrow(/terminal socket disconnected/);
    socket.close();
  });

  it("rejects what was in flight on the wire that died", async () => {
    const { socket } = standing();
    const carrier = fakeCarrier({ deafTo: ["term.list"] });
    await socket.peer(carrier);
    const pending = socket.listTerminals({ project_id: "p" });
    pending.catch(() => {});
    await tick();

    socket.peer(null);

    await expect(pending).rejects.toThrow(/terminal socket/);
    socket.close();
  });

  it("drops the wire whose liveness ping went unanswered", async () => {
    vi.useFakeTimers();
    try {
      const { socket } = standing();
      const carrier = fakeCarrier({ deafTo: ["ping"] });
      carrier.onClose(() => socket.peer(null));
      await socket.peer(carrier);

      // Two seconds of silence buys a ping; three more without an answer says
      // the wire is gone.
      await vi.advanceTimersByTimeAsync(6000);

      expect(carrier.closed).toBe(true);
      socket.close();
    } finally {
      vi.useRealTimers();
    }
  });

  // The shells move machine by being given that machine's session and that
  // machine's channel. The session is what the frames are sealed under, so a
  // channel adopted before the session it belongs to must be re-taken under it.
  it("takes the same wire under a newly adopted session, and re-attaches there", async () => {
    const { socket } = standing(terminalSession("sess-1", "dev-a"));
    const carrier = fakeCarrier();
    await socket.peer(carrier);
    await withTerminal(socket, carrier);

    await socket.adoptTerminalSession(terminalSession("sess-2", "dev-b"));

    expect(socket.deviceId).toBe("dev-b");
    expect(carrier.calls("term.attach")).toHaveLength(2);
    expect(carrier.sent.at(-1).key).toBe("key-sess-2");
    // …and the screen still reads, because it reads under the key it now holds.
    const typing = socket.input("term-1", "x");
    await expect(typing).resolves.toBeUndefined();
    socket.close();
  });

  // The manager hands the channel over as soon as the device's peer link has
  // one, which can be before the mint it asked for has landed. A wire with no
  // session carries nothing: saying otherwise would resolve a pane's wait and
  // then refuse the attach it made.
  it("says nothing about a wire it was handed before its session", async () => {
    const socket = new TerminalSocket({ transport: fakeTransport });
    const statuses = [];
    socket.onStatus((status) => statuses.push(status));
    const carrier = fakeCarrier();

    await socket.peer(carrier);
    const waiting = socket.whenConnected();
    expect(statuses).toEqual([]);

    await socket.adoptTerminalSession(terminalSession("sess-1"));

    await expect(waiting).resolves.toBeUndefined();
    expect(statuses).toEqual(["connecting", "connected"]);
    await expect(socket.input("term-1", "x")).resolves.toBeUndefined();
    socket.close();
  });

  it("fails a call whose envelope never crossed the wire, rather than waiting out its timeout", async () => {
    const { socket } = standing();
    await socket.peer(fakeCarrier({ sendFails: "the channel closed" }));

    await expect(socket.input("term-1", "x")).rejects.toThrow(/the channel closed/);
    socket.close();
  });
});
