import { describe, it, expect } from "vitest";
import { TerminalSocket } from "../src/terminal/session.js";

class FakeWebSocket {
  constructor(url) {
    this.url = url;
    this.sent = [];
    this.listeners = {};
    FakeWebSocket.instances.push(this);
  }
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
  send(text) { this.sent.push(JSON.parse(text)); }
  close() { this.emit("close", {}); }
  emit(type, event = {}) { (this.listeners[type] || []).forEach((fn) => fn(event)); }
  serverSend(obj) { this.emit("message", { data: JSON.stringify(obj) }); }
}
FakeWebSocket.instances = [];

const fakeTransport = {
  ready: async () => {},
  createSessionInit: async ({ sessionId, deviceId }) => ({
    sessionKeyB64: `key-${deviceId}`,
    sessionInit: { session_id: sessionId, device_id: deviceId },
  }),
  openSessionAccept: async () => {},
  encryptFrame: async ({ outerFields, frameFields }) => ({ outerFields, frameFields }),
  decryptEnvelope: async ({ envelope }) => ({ payload: envelope.frameFields.payload }),
};

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const b64 = (s) => btoa(s);
const dec = (bytes) => new TextDecoder().decode(bytes);

function makeSocket(overrides = {}) {
  return new TerminalSocket({
    url: "ws://relay.test",
    transport: fakeTransport,
    WebSocketImpl: FakeWebSocket,
    getToken: async () => "tok-9",
    getPinnedDeviceKey: async (deviceId) => `pk-${deviceId.slice(-1)}`,
    preferDeviceId: () => "dev-b",
    ...overrides,
  });
}

// Drive the E2EE handshake on `ws` to a connected state; returns the session_init.
async function handshake(ws, { deviceId = "dev-b", pinned = "pk-b" } = {}) {
  ws.emit("open");
  await tick();
  ws.serverSend({ type: "authenticated" });
  ws.serverSend({ type: "device_key", device_id: deviceId, transport_public_key: pinned });
  await tick();
  const init = ws.sent.find((m) => m.type === "session_init");
  ws.serverSend({ type: "session_accept", session_id: init.session_id, envelope: {} });
  await tick();
  return init;
}

const lastPayload = (ws) => ws.sent.at(-1).envelope.frameFields.payload;
const respond = (ws, init, id, result) =>
  ws.serverSend({
    type: "e2ee_envelope",
    session_id: init.session_id,
    envelope: { frameFields: { payload: { id, ok: true, result } } },
  });
const rejectCall = (ws, init, id, error) =>
  ws.serverSend({
    type: "e2ee_envelope",
    session_id: init.session_id,
    envelope: { frameFields: { payload: { id, ok: false, error } } },
  });
const push = (ws, init, payload) =>
  ws.serverSend({ type: "e2ee_envelope", session_id: init.session_id, envelope: { frameFields: { payload } } });

async function connected(overrides) {
  FakeWebSocket.instances.length = 0;
  const socket = makeSocket(overrides);
  const started = socket.start();
  started.catch(() => {});
  await tick();
  const ws = FakeWebSocket.instances.at(-1);
  const init = await handshake(ws);
  await started;
  return { socket, ws, init };
}

describe("TerminalSocket", () => {
  it("completes the handshake without attaching anything (connecting no longer implies attach)", async () => {
    const { ws } = await connected();
    // No term.attach was sent as part of connecting — the socket just came up.
    const methods = ws.sent
      .filter((m) => m.type === "e2ee_envelope")
      .map((m) => m.envelope.frameFields.payload.method);
    expect(methods).not.toContain("term.attach");
  });

  it("seals to the api-pinned key and hard-fails on a mismatched relay-pushed key", async () => {
    FakeWebSocket.instances.length = 0;
    const sealedTo = [];
    const spyTransport = {
      ...fakeTransport,
      createSessionInit: async (args) => {
        sealedTo.push(args.deviceTransportPublicKeyB64);
        return fakeTransport.createSessionInit(args);
      },
    };
    const socket = makeSocket({ transport: spyTransport, getPinnedDeviceKey: async () => "pk-genuine", preferDeviceId: () => "dev-a" });
    const started = socket.start();
    started.catch(() => {});
    await tick();
    const ws = FakeWebSocket.instances[0];
    ws.emit("open");
    await tick();
    ws.serverSend({ type: "authenticated" });
    ws.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-attacker" });
    await expect(started).rejects.toThrow(/does not match/);
    expect(sealedTo).toEqual([]);
    expect(ws.sent.find((m) => m.type === "session_init")).toBeUndefined();
    socket.close();
  });

  it("attachTerminal registers and sends term.attach {term_id, cols, rows}; output/reset dedupe per term", async () => {
    const { socket, ws, init } = await connected();
    const outputs = [];
    const snapshots = [];
    const attaching = socket.attachTerminal("term-3", {
      cols: 100, rows: 30,
      onOutput: (b) => outputs.push(dec(b)),
      onSnapshot: (b) => snapshots.push(dec(b)),
    });
    attaching.catch(() => {});
    await tick();
    const attach = lastPayload(ws);
    expect(attach.method).toBe("term.attach");
    expect(attach.params).toEqual({ term_id: "term-3", cols: 100, rows: 30 });
    respond(ws, init, attach.id, { snapshot: b64("SCREEN"), cursor: 5 });
    await attaching;
    expect(snapshots).toEqual(["SCREEN"]);

    push(ws, init, { type: "term.output", term_id: "term-3", cursor: 6, data: b64("live") });
    push(ws, init, { type: "term.output", term_id: "term-3", cursor: 4, data: b64("stale") });
    push(ws, init, { type: "term.reset", term_id: "term-3", cursor: 8, data: b64("RESET") });
    // A frame for an id we never registered is ignored.
    push(ws, init, { type: "term.output", term_id: "term-99", cursor: 99, data: b64("ghost") });
    await tick();
    expect(outputs).toEqual(["live"]);
    expect(snapshots).toEqual(["SCREEN", "RESET"]);
    socket.close();
  });

  it("demuxes two terminals independently over one socket", async () => {
    const { socket, ws, init } = await connected();
    const a = [];
    const b = [];
    const at1 = socket.attachTerminal("term-1", { cols: 80, rows: 24, onOutput: (x) => a.push(dec(x)), onSnapshot: () => {} });
    at1.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await at1;
    const at2 = socket.attachTerminal("term-2", { cols: 80, rows: 24, onOutput: (x) => b.push(dec(x)), onSnapshot: () => {} });
    at2.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await at2;

    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 1, data: b64("one") });
    push(ws, init, { type: "term.output", term_id: "term-2", cursor: 1, data: b64("two") });
    await tick();
    expect(a).toEqual(["one"]);
    expect(b).toEqual(["two"]);
    socket.close();
  });

  it("term.closed deregisters a user terminal but retains an agent screen on session-end", async () => {
    const { socket, ws, init } = await connected();
    const userClosed = [];
    const agentClosed = [];
    const agentOut = [];

    const at1 = socket.attachTerminal("term-1", { cols: 80, rows: 24, onSnapshot: () => {}, onClosed: (r) => userClosed.push(r) });
    at1.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await at1;

    const at2 = socket.attachAgent("task-5", {
      cols: 120, rows: 40,
      onSnapshot: () => {}, onOutput: (x) => agentOut.push(dec(x)),
      onClosed: (r) => agentClosed.push(r), onLive: () => {},
    });
    at2.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0, live: true });
    await at2;

    // User terminal exits → onClosed + deregister (a later frame is ignored).
    push(ws, init, { type: "term.closed", term_id: "term-1", reason: "exited" });
    await tick();
    expect(userClosed).toEqual(["exited"]);
    const outsBefore = agentOut.length;
    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 9, data: b64("zombie") });
    await tick();

    // Agent session ends → onClosed fires but the screen stays registered.
    push(ws, init, { type: "term.closed", term_id: "agent:task-5", reason: "agent_session_ended" });
    await tick();
    expect(agentClosed).toEqual(["agent_session_ended"]);
    // A new session's output still routes to the retained agent screen.
    push(ws, init, { type: "term.output", term_id: "agent:task-5", cursor: 1, data: b64("next") });
    await tick();
    expect(agentOut.slice(outsBefore)).toEqual(["next"]);
    socket.close();
  });

  it("reconnect re-attaches every registered terminal and resets cursors from the new snapshots", async () => {
    const { socket, ws, init } = await connected();
    const userSnaps = [];
    const agentSnaps = [];
    const userClosed = [];
    const lives = [];

    const at1 = socket.attachTerminal("term-1", { cols: 80, rows: 24, onSnapshot: (b) => userSnaps.push(dec(b)), onClosed: (r) => userClosed.push(r) });
    at1.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64("U1"), cursor: 3 });
    await at1;

    const at2 = socket.attachAgent("task-5", { cols: 120, rows: 40, onSnapshot: (b) => agentSnaps.push(dec(b)), onLive: (l) => lives.push(l) });
    at2.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64("A1"), cursor: 10, live: true });
    await at2;
    expect(lives).toEqual([true]);

    // Drop → the socket reconnects with backoff and re-attaches every term.
    socket.simulateDrop();
    await new Promise((r) => setTimeout(r, 600));
    const ws2 = FakeWebSocket.instances.at(-1);
    expect(ws2).not.toBe(ws);
    const init2 = await handshake(ws2);

    // First re-attach (user term-1) → rejected as unknown → reaped + deregistered.
    let p = lastPayload(ws2);
    expect(p.method).toBe("term.attach");
    expect(p.params).toEqual({ term_id: "term-1", cols: 80, rows: 24 });
    rejectCall(ws2, init2, p.id, "unknown term_id");
    await tick();
    expect(userClosed).toEqual(["reaped"]);

    // Then the agent re-attaches via agent.attach and updates onLive.
    await tick();
    p = lastPayload(ws2);
    expect(p.method).toBe("agent.attach");
    expect(p.params).toEqual({ task_id: "task-5", cols: 120, rows: 40 });
    respond(ws2, init2, p.id, { snapshot: b64("A2"), cursor: 20, live: false });
    await tick();
    expect(agentSnaps).toEqual(["A1", "A2"]);
    expect(lives).toEqual([true, false]);
    socket.close();
  });

  it("create/list/close carry the scope spread and the right shapes", async () => {
    const { socket, ws, init } = await connected();

    const creating = socket.createTerminal({ project_id: "proj-1" }, 80, 24);
    creating.catch(() => {});
    await tick();
    let p = lastPayload(ws);
    expect(p.method).toBe("term.create");
    expect(p.params).toEqual({ project_id: "proj-1", cols: 80, rows: 24 });
    respond(ws, init, p.id, { term_id: "term-7", cols: 80, rows: 24 });
    expect(await creating).toEqual({ term_id: "term-7", cols: 80, rows: 24 });

    const listing = socket.listTerminals({ task_id: "task-3" });
    listing.catch(() => {});
    await tick();
    p = lastPayload(ws);
    expect(p.method).toBe("term.list");
    expect(p.params).toEqual({ task_id: "task-3" });
    respond(ws, init, p.id, { terminals: [{ term_id: "term-1", cols: 80, rows: 24 }] });
    expect(await listing).toEqual([{ term_id: "term-1", cols: 80, rows: 24 }]);

    // Register then close: the entry deregisters even though the server round-trips.
    const at1 = socket.attachTerminal("term-1", { cols: 80, rows: 24, onSnapshot: () => {} });
    at1.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await at1;
    const closing = socket.closeTerminal("term-1");
    closing.catch(() => {});
    await tick();
    p = lastPayload(ws);
    expect(p.method).toBe("term.close");
    expect(p.params).toEqual({ term_id: "term-1" });
    respond(ws, init, p.id, { ok: true });
    await closing;
    // A push to the now-closed id is ignored.
    const out = [];
    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 5, data: b64("x") });
    await tick();
    expect(out).toEqual([]);
    socket.close();
  });
});
