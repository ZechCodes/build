// One device's rendezvous: a relay socket that is open only while something is
// negotiating (spec rules 4 and 5).
//
// It mints sessions — any number of them, on the one socket — hands each a
// signaling carrier, and closes. There is no reconnect loop and no presence on
// it: a socket that goes says so, and the caller decides what that means.

import { describe, it, expect, vi } from "vitest";
import { createRelayRendezvous } from "../src/core/rendezvous.js";

class FakeWebSocket {
  constructor(url) {
    this.url = url;
    this.sent = [];
    this.listeners = {};
    this.readyState = 1; // OPEN
    FakeWebSocket.instances.push(this);
  }
  addEventListener(type, fn) {
    (this.listeners[type] ||= []).push(fn);
  }
  send(text) {
    if (this.readyState !== 1) throw new Error("WebSocket is already in CLOSING or CLOSED state.");
    this.sent.push(JSON.parse(text));
  }
  close() {
    if (this.readyState === 3) return;
    this.readyState = 3; // CLOSED
    this.emit("close", {});
  }
  emit(type, event = {}) {
    (this.listeners[type] || []).forEach((fn) => fn(event));
  }
  serverSend(obj) {
    this.emit("message", { data: JSON.stringify(obj) });
  }
  find(type) {
    return this.sent.find((m) => m.type === type);
  }
  all(type) {
    return this.sent.filter((m) => m.type === type);
  }
}
FakeWebSocket.instances = [];

const fakeTransport = {
  ready: async () => {},
  createSessionInit: async ({ sessionId, deviceId, sessionKeyB64 }) => ({
    sessionKeyB64: sessionKeyB64 || `key-${sessionId}`,
    sessionInit: { session_id: sessionId, device_id: deviceId },
  }),
  openSessionAccept: async () => {},
};

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function stand(overrides = {}) {
  FakeWebSocket.instances.length = 0;
  const tokens = [];
  const rendezvous = createRelayRendezvous({
    deviceId: "dev-a",
    relayUrl: "wss://relay.test",
    transport: fakeTransport,
    WebSocketImpl: FakeWebSocket,
    fetchToken: async () => {
      tokens.push(`tok-${tokens.length + 1}`);
      return tokens.at(-1);
    },
    getPinnedDeviceKey: async (deviceId) => `pk-${deviceId}`,
    ...overrides,
  });
  return { rendezvous, tokens, socket: () => FakeWebSocket.instances.at(-1) };
}

/** The relay's half: the socket comes up, and the device answers every
 *  `session_init` it is handed. */
async function answering(stood, { accept = true } = {}) {
  await tick();
  const ws = stood.socket();
  ws.emit("open");
  await tick();
  if (accept) {
    for (const init of ws.all("session_init")) {
      ws.serverSend({ type: "session_accept", session_id: init.session_id, envelope: { for: init.session_id } });
    }
  }
  await tick();
  return ws;
}

/** A rendezvous with one session minted on it. */
async function minted(overrides = {}) {
  const stood = stand(overrides);
  const opening = stood.rendezvous.mint({});
  opening.catch(() => {});
  const ws = await answering(stood);
  return { ...stood, ws, session: await opening };
}

describe("createRelayRendezvous", () => {
  it("opens one authenticated socket and keeps it while it is open", async () => {
    const { rendezvous, tokens, socket } = stand();
    const opening = rendezvous.open();
    const second = rendezvous.open();
    await tick();
    socket().emit("open");
    await opening;
    await second;

    expect(FakeWebSocket.instances).toHaveLength(1);
    expect(socket().url).toBe("wss://relay.test/ws/client");
    expect(socket().sent[0]).toEqual({ type: "authenticate", token: "tok-1" });
    expect(tokens).toEqual(["tok-1"]); // one attempt, one gateway token
    expect(rendezvous.isOpen()).toBe(true);
    rendezvous.close();
  });

  it("mints a session without waiting for the relay to announce the device", async () => {
    const { ws, session, rendezvous } = await minted();

    const init = ws.find("session_init");
    expect(init.route_to).toBe("device:dev-a");
    expect(init.session_init.device_id).toBe("dev-a");
    expect(session).toEqual({ sessionId: init.session_id, sessionKeyB64: `key-${init.session_id}`, deviceId: "dev-a" });
    rendezvous.close();
  });

  it("seals the session key to the api-pinned transport key", async () => {
    const sealedTo = [];
    const { rendezvous } = await minted({
      transport: {
        ...fakeTransport,
        createSessionInit: async (args) => {
          sealedTo.push(args.deviceTransportPublicKeyB64);
          return fakeTransport.createSessionInit(args);
        },
      },
    });
    expect(sealedTo).toEqual(["pk-dev-a"]);
    rendezvous.close();
  });

  it("refuses a device the api knows no transport key for", async () => {
    const stood = stand({ getPinnedDeviceKey: async () => null });
    const refused = stood.rendezvous.mint({}).catch((error) => error);
    await answering(stood);
    const error = await refused;
    expect(error.message).toMatch(/no pinned transport key/);
    expect(error.securityCritical).toBe(true);
    expect(stood.socket().find("session_init")).toBeUndefined();
  });

  it("requires a pinned-key source instead of trusting whatever the relay says", () => {
    expect(() =>
      createRelayRendezvous({
        deviceId: "dev-a",
        relayUrl: "wss://relay.test",
        transport: fakeTransport,
        WebSocketImpl: FakeWebSocket,
        fetchToken: async () => "tok",
      }),
    ).toThrow(/getPinnedDeviceKey/);
  });

  it("mints a second session on the same socket, each with its own key", async () => {
    const stood = stand();
    const both = Promise.all([stood.rendezvous.mint({}), stood.rendezvous.mint({})]);
    both.catch(() => {});
    const ws = await answering(stood);
    const [app, term] = await both;

    expect(FakeWebSocket.instances).toHaveLength(1);
    expect(ws.all("session_init")).toHaveLength(2);
    expect(app.sessionId).not.toBe(term.sessionId);
    expect(app.sessionKeyB64).not.toBe(term.sessionKeyB64);
    stood.rendezvous.close();
  });

  // Rule 5's multiplexing: several sessions negotiate on the one socket, and
  // the accept the relay forwards is routed by the id it routed by. What a mint
  // returns is its own closure's, so it says nothing about which answer settled
  // it — the envelope each session UNSEALED is what pins the routing, and a
  // swapped one is a device's sealed reply opened under another session's key.
  it("takes each session_accept to the mint that is waiting for it", async () => {
    const opened = [];
    const stood = stand({
      transport: {
        ...fakeTransport,
        openSessionAccept: async ({ sessionKeyB64, envelope }) => {
          opened.push({ sessionKeyB64, envelope });
        },
      },
    });
    const app = stood.rendezvous.mint({});
    app.catch(() => {});
    await tick();
    const ws = stood.socket();
    ws.emit("open");
    await tick();
    const term = stood.rendezvous.mint({});
    term.catch(() => {});
    await tick();
    const [first, second] = ws.all("session_init");

    // Out of order, as two devices' answers may well arrive, each envelope
    // naming the session it answers.
    ws.serverSend({ type: "session_accept", session_id: second.session_id, envelope: { for: second.session_id } });
    ws.serverSend({ type: "session_accept", session_id: first.session_id, envelope: { for: first.session_id } });

    expect((await app).sessionId).toBe(first.session_id);
    expect((await term).sessionId).toBe(second.session_id);

    const openedWith = new Map(opened.map(({ sessionKeyB64, envelope }) => [sessionKeyB64, envelope.for]));
    expect(openedWith.size).toBe(2);
    expect(openedWith.get(`key-${first.session_id}`)).toBe(first.session_id);
    expect(openedWith.get(`key-${second.session_id}`)).toBe(second.session_id);
    stood.rendezvous.close();
  });

  // Two things can ask for the same session at once — an ICE restart and a
  // reader's Retry, say. One waiter per session id is all the relay's routing
  // can answer, so the second is refused rather than quietly taking the first
  // one's slot: the first's accept deadline would then delete the second's
  // waiter and the device's answer would reach nobody.
  it("refuses a second mint of a session id one is already waiting on", async () => {
    const stood = stand({ acceptTimeoutMs: 40 });
    const first = stood.rendezvous.mint({ sessionId: "sess-held", sessionKeyB64: "key-held" });
    first.catch(() => {});
    const ws = await answering(stood, { accept: false });

    await expect(stood.rendezvous.mint({ sessionId: "sess-held", sessionKeyB64: "key-held" })).rejects.toThrow(
      /already being minted/,
    );

    // And the one that was already waiting is untouched: its init stands, and
    // the device's answer still settles it.
    expect(ws.all("session_init")).toHaveLength(1);
    ws.serverSend({ type: "session_accept", session_id: "sess-held", envelope: { for: "sess-held" } });
    await expect(first).resolves.toEqual({ sessionId: "sess-held", sessionKeyB64: "key-held", deviceId: "dev-a" });
    stood.rendezvous.close();
  });

  it("re-attaches a session by presenting the id and key it already has", async () => {
    const { rendezvous, session, ws } = await minted();
    rendezvous.close();

    const again = rendezvous.mint({ sessionId: session.sessionId, sessionKeyB64: session.sessionKeyB64 });
    again.catch(() => {});
    const next = await answering({ socket: () => FakeWebSocket.instances.at(-1) });

    expect(next).not.toBe(ws);
    expect(next.find("session_init").session_id).toBe(session.sessionId);
    await expect(again).resolves.toEqual(session);
    rendezvous.close();
  });

  it("reopens on a mint when it is closed, with a fresh gateway token", async () => {
    const { rendezvous, tokens } = await minted();
    rendezvous.close();
    expect(rendezvous.isOpen()).toBe(false);

    const again = rendezvous.mint({});
    again.catch(() => {});
    await answering({ socket: () => FakeWebSocket.instances.at(-1) });
    await again;

    expect(FakeWebSocket.instances).toHaveLength(2);
    expect(tokens).toEqual(["tok-1", "tok-2"]);
    rendezvous.close();
  });

  it("gives up on a device that does not answer the session it was offered", async () => {
    const stood = stand({ acceptTimeoutMs: 20 });
    const refused = stood.rendezvous.mint({}).catch((error) => error);
    await answering(stood, { accept: false });
    expect((await refused).message).toMatch(/device did not answer/);
    stood.rendezvous.close();
  });

  // Rule 4: the relay is held open only while something is negotiating, so a
  // dial nobody is waiting for any more must not land an authenticated socket
  // with no owner — there is nothing that would ever close it.
  it("cancels a dial the caller closed while the gateway token was in flight", async () => {
    let handOver;
    const stood = stand({ fetchToken: () => new Promise((settle) => { handOver = settle; }) });
    const opening = stood.rendezvous.open().catch((error) => error);
    await tick();

    stood.rendezvous.close();
    handOver("tok-late");

    expect((await opening).message).toMatch(/closed/);
    expect(FakeWebSocket.instances).toHaveLength(0); // no socket was ever dialled
    expect(stood.rendezvous.isOpen()).toBe(false);
  });

  it("dials again after a cancelled one, rather than holding the attempt that was called off", async () => {
    let handOver;
    const stood = stand({
      fetchToken: () => (handOver ? Promise.resolve("tok-2") : new Promise((settle) => { handOver = settle; })),
    });
    const cancelled = stood.rendezvous.open().catch((error) => error);
    await tick();
    stood.rendezvous.close();
    handOver("tok-1");
    await cancelled;

    const again = stood.rendezvous.open();
    await tick();
    stood.socket().emit("open");
    await again;

    expect(FakeWebSocket.instances).toHaveLength(1);
    expect(stood.socket().sent[0]).toEqual({ type: "authenticate", token: "tok-2" });
    stood.rendezvous.close();
  });

  it("gives up on a relay that does not answer at all", async () => {
    const stood = stand({ openTimeoutMs: 20 });
    const refused = stood.rendezvous.open().catch((error) => error);
    await tick();
    expect((await refused).message).toMatch(/relay/);
    expect(stood.rendezvous.isOpen()).toBe(false);
  });

  it("hands a signaling carrier for one session that sends and reads on the shared socket", async () => {
    const { rendezvous, session, ws } = await minted();
    const carrier = rendezvous.signalCarrier(session.sessionId);
    const read = [];
    carrier.onEnvelope((envelope) => read.push(envelope));

    carrier.send({ nonce: "n" });
    expect(ws.sent.at(-1)).toEqual({ type: "e2ee_envelope", session_id: session.sessionId, envelope: { nonce: "n" } });

    ws.serverSend({ type: "e2ee_envelope", session_id: "sess-somebody-else", envelope: { nonce: "x" } });
    ws.serverSend({ type: "e2ee_envelope", session_id: session.sessionId, envelope: { nonce: "y" } });
    expect(read).toEqual([{ nonce: "y" }]);
    rendezvous.close();
  });

  it("has no signaling carrier to give while it is closed", async () => {
    const { rendezvous, session } = await minted();
    rendezvous.close();
    expect(() => rendezvous.signalCarrier(session.sessionId)).toThrow(/not open/);
  });

  it("ends every signaling carrier when it closes, and the socket with them", async () => {
    const { rendezvous, session, ws } = await minted();
    const carrier = rendezvous.signalCarrier(session.sessionId);
    const other = rendezvous.signalCarrier("sess-term");
    const ended = [];
    carrier.onClose(() => ended.push("app"));
    other.onClose(() => ended.push("term"));

    rendezvous.close();

    expect(ws.readyState).toBe(3);
    expect(ended.sort()).toEqual(["app", "term"]);
    expect(rendezvous.isOpen()).toBe(false);
  });

  it("says nothing about a close the caller asked for", async () => {
    const { rendezvous } = await minted();
    const closed = vi.fn();
    rendezvous.onClosed(closed);

    rendezvous.close();

    expect(closed).not.toHaveBeenCalled();
  });

  // Rule 4: the relay is a rendezvous, not a connection. Nothing here waits for
  // it to come back — the caller reopens it when it next has something to
  // negotiate, and decides what a socket that went mid-negotiation costs.
  it("reports a socket that went on its own and never reconnects", async () => {
    const { rendezvous, ws } = await minted();
    const closed = vi.fn();
    rendezvous.onClosed(closed);

    ws.close();
    await tick();
    await tick();

    expect(closed).toHaveBeenCalledTimes(1);
    expect(rendezvous.isOpen()).toBe(false);
    expect(FakeWebSocket.instances).toHaveLength(1); // nothing dialled again
  });

  it("rejects the mint that was in flight when the socket went", async () => {
    const stood = stand();
    const pending = stood.rendezvous.mint({}).catch((error) => error);
    const ws = await answering(stood, { accept: false });

    ws.close();

    expect((await pending).message).toMatch(/closed/);
  });

  it("stops delivering to a signaling carrier once the socket has gone", async () => {
    const { rendezvous, session, ws } = await minted();
    const carrier = rendezvous.signalCarrier(session.sessionId);
    const ended = vi.fn();
    carrier.onClose(ended);

    ws.close();
    await tick();

    expect(ended).toHaveBeenCalledTimes(1);
    expect(() => carrier.send({ nonce: "n" })).toThrow(/closed/);
  });
});
