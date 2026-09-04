// The terminal socket over two carriers: the relay socket it handshakes on, and
// the `term` DataChannel it migrates to. Its public methods do not change —
// what changes is the wire underneath them.

import { describe, it, expect, vi } from "vitest";
import { TerminalSocket } from "../src/terminal/session.js";

class FakeWebSocket {
  constructor(url) {
    this.url = url;
    this.sent = [];
    this.listeners = {};
    FakeWebSocket.instances.push(this);
  }
  addEventListener(type, fn) {
    (this.listeners[type] ||= []).push(fn);
  }
  send(text) {
    this.sent.push(JSON.parse(text));
  }
  close() {
    this.emit("close", {});
  }
  emit(type, event = {}) {
    (this.listeners[type] || []).forEach((fn) => fn(event));
  }
  serverSend(obj) {
    this.emit("message", { data: JSON.stringify(obj) });
  }
}
FakeWebSocket.instances = [];

const fakeTransport = {
  ready: async () => {},
  createSessionInit: async ({ sessionId, deviceId }) => ({
    sessionKeyB64: `key-${sessionId}`,
    sessionInit: { session_id: sessionId, device_id: deviceId },
  }),
  openSessionAccept: async () => {},
  encryptFrame: async ({ outerFields, frameFields }) => ({ outerFields, frameFields }),
  decryptEnvelope: async ({ envelope }) => ({ payload: envelope.frameFields.payload }),
};

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = async () => {
  for (let i = 0; i < 8; i++) await tick();
};

/** A DataChannel carrier the test drives, answering every RPC the way the
 *  bridge would so a migration can finish. */
function fakeCarrier({ deafTo = [] } = {}) {
  let onEnvelope = () => {};
  let onClose = () => {};
  const carrier = {
    sent: [],
    closed: false,
    send(envelope) {
      carrier.sent.push(envelope);
      const { method, id } = envelope.frameFields.payload;
      if (deafTo.includes(method)) return;
      const result =
        method === "term.attach" || method === "agent.attach"
          ? { snapshot: "", cursor: 7, term_id: envelope.frameFields.payload.params.term_id }
          : {};
      Promise.resolve().then(() => onEnvelope({ frameFields: { payload: { id, ok: true, result } } }));
    },
    onEnvelope: (fn) => (onEnvelope = fn),
    onClose: (fn) => (onClose = fn),
    close: () => {
      carrier.closed = true;
      onClose();
    },
    push: (payload) => onEnvelope({ frameFields: { payload } }),
    calls: (method) => carrier.sent.map((e) => e.frameFields.payload).filter((p) => p.method === method),
  };
  return carrier;
}

async function connected(settle = tick) {
  FakeWebSocket.instances.length = 0;
  const socket = new TerminalSocket({
    url: "wss://relay.test",
    transport: fakeTransport,
    WebSocketImpl: FakeWebSocket,
    getToken: async () => "tok",
    getPinnedDeviceKey: async () => "pk-b",
    preferDeviceId: () => "dev-b",
  });
  const statuses = [];
  socket.onStatus((s) => statuses.push(s));
  const started = socket.start();
  started.catch(() => {});
  await settle();
  const ws = FakeWebSocket.instances.at(-1);
  ws.emit("open");
  await settle();
  ws.serverSend({ type: "authenticated" });
  ws.serverSend({ type: "device_key", device_id: "dev-b", transport_public_key: "pk-b" });
  await settle();
  const init = ws.sent.find((m) => m.type === "session_init");
  ws.serverSend({ type: "session_accept", session_id: init.session_id, envelope: {} });
  await started;
  return { socket, ws, init, statuses };
}

const respond = (ws, id, result) =>
  ws.serverSend({ type: "e2ee_envelope", envelope: { frameFields: { payload: { id, ok: true, result } } } });
const lastPayload = (ws) => ws.sent.at(-1).envelope.frameFields.payload;

/** One attached user terminal, its screen collected. */
async function withTerminal(socket, ws) {
  const screen = { output: [], snapshots: [] };
  const attaching = socket.attachTerminal("term-1", {
    onOutput: (bytes) => screen.output.push(new TextDecoder().decode(bytes)),
    onSnapshot: (bytes) => screen.snapshots.push(new TextDecoder().decode(bytes)),
  });
  await tick();
  respond(ws, lastPayload(ws).id, { snapshot: "", cursor: 0 });
  await attaching;
  return screen;
}

describe("a terminal socket that rides two carriers", () => {
  it("re-attaches every open terminal over the channel and sends there from then on", async () => {
    const { socket, ws } = await connected();
    await withTerminal(socket, ws);
    const carrier = fakeCarrier();

    await socket.peer(carrier);
    expect(carrier.calls("term.attach").map((p) => p.params.term_id)).toEqual(["term-1"]);

    const sentOnRelay = ws.sent.length;
    await socket.input("term-1", "ls\n");
    expect(carrier.calls("term.input")).toHaveLength(1);
    expect(ws.sent).toHaveLength(sentOnRelay);
    socket.close();
  });

  it("applies the channel's pushes to the screen they name", async () => {
    const { socket, ws } = await connected();
    const screen = await withTerminal(socket, ws);
    const carrier = fakeCarrier();
    await socket.peer(carrier);

    carrier.push({ type: "term.output", term_id: "term-1", cursor: 9, data: btoa("hi") });
    await settle();
    expect(screen.output).toEqual(["hi"]);
    socket.close();
  });

  it("stays connected when the relay drops under a live channel, and reconnects it quietly", async () => {
    const { socket, ws, statuses } = await connected();
    await withTerminal(socket, ws);
    const carrier = fakeCarrier();
    await socket.peer(carrier);
    statuses.length = 0;
    const sockets = FakeWebSocket.instances.length;

    ws.close();
    await settle();
    expect(statuses).not.toContain("disconnected");
    await expect(socket.input("term-1", "x")).resolves.toBeUndefined(); // the channel answered

    // The relay still comes back on its own — signaling and presence need it.
    const deadline = Date.now() + 4000;
    while (FakeWebSocket.instances.length <= sockets && Date.now() < deadline) await tick();
    expect(FakeWebSocket.instances.length).toBeGreaterThan(sockets);
    socket.close();
  });

  it("falls back to the relay when the channel is lost, re-attaching there", async () => {
    const { socket, ws } = await connected();
    await withTerminal(socket, ws);
    const carrier = fakeCarrier();
    await socket.peer(carrier);

    const falling = socket.peer(null);
    await tick();
    expect(lastPayload(ws).method).toBe("term.attach");
    respond(ws, lastPayload(ws).id, { snapshot: "", cursor: 0 });
    await falling;
    const typing = socket.input("term-1", "y");
    await tick();
    expect(lastPayload(ws).method).toBe("term.input");
    respond(ws, lastPayload(ws).id, {});
    await typing;
    socket.close();
  });

  it("reports disconnected only once no carrier is left", async () => {
    const { socket, ws, statuses } = await connected();
    await withTerminal(socket, ws);
    const carrier = fakeCarrier();
    await socket.peer(carrier);
    ws.close();
    await settle();
    expect(statuses).not.toContain("disconnected");

    socket.peer(null);
    await tick();
    expect(statuses).toContain("disconnected");
    socket.close();
  });

  it("drops the wire whose liveness ping went unanswered, not the other one", async () => {
    vi.useFakeTimers();
    try {
      const settle = () => vi.advanceTimersByTimeAsync(0);
      const { socket, ws } = await connected(settle);
      const carrier = fakeCarrier({ deafTo: ["ping"] });
      // Its owner is what hands the channel over and takes it back, exactly as
      // terminal/manager.js does for the peer link's `term` half.
      carrier.onClose(() => socket.peer(null));
      await socket.peer(carrier);
      const sockets = FakeWebSocket.instances.length;

      // Two seconds of silence buys a ping; three more without an answer says
      // the wire is gone — and the wire is the channel, not the relay socket.
      await vi.advanceTimersByTimeAsync(6000);

      expect(carrier.closed).toBe(true);
      expect(FakeWebSocket.instances).toHaveLength(sockets);
      const listing = socket.listTerminals({ project_id: "p" });
      listing.catch(() => {});
      await settle();
      expect(ws.sent.at(-1).envelope.frameFields.payload.method).toBe("term.list");
      socket.close();
    } finally {
      vi.useRealTimers();
    }
  });

  it("rejects what was in flight on the wire that died, once nothing is carrying", async () => {
    const { socket, ws } = await connected();
    await withTerminal(socket, ws);
    const pending = socket.listTerminals({ project_id: "p" });
    pending.catch(() => {});
    await tick();
    ws.close();
    await expect(pending).rejects.toThrow(/terminal socket/);
    socket.close();
  });
});
