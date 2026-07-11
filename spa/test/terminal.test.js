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

  it("applies an equal-cursor term.reset (the agent pump's start-of-session wipe)", async () => {
    const { socket, ws, init } = await connected();
    const snaps = [];
    const outputs = [];
    // Attach to a task whose previous session's screen is retained: the response
    // carries cursor == total. The pump the attach spawned then wipes with
    // term.reset{cursor: total} — resets never advance `total`, so the cursor is
    // EQUAL to the attach cursor. The wipe must still apply or the new session's
    // bytes garble over the old screen.
    const at = socket.attachAgent("task-9", {
      cols: 120, rows: 40,
      onSnapshot: (b) => snaps.push(dec(b)),
      onOutput: (b) => outputs.push(dec(b)),
      onLive: () => {},
    });
    at.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64("OLD SCREEN"), cursor: 42, live: true });
    await at;
    expect(snaps).toEqual(["OLD SCREEN"]);

    push(ws, init, { type: "term.reset", term_id: "agent:task-9", cursor: 42, data: b64("") });
    push(ws, init, { type: "term.output", term_id: "agent:task-9", cursor: 50, data: b64("new session") });
    await tick();
    expect(snaps).toEqual(["OLD SCREEN", ""]);
    expect(outputs).toEqual(["new session"]);
    socket.close();
  });

  it("a streamed frame on an idle agent screen reports the session live again", async () => {
    const { socket, ws, init } = await connected();
    const lives = [];
    const closed = [];
    const at = socket.attachAgent("task-4", {
      cols: 120, rows: 40,
      onSnapshot: () => {}, onOutput: () => {},
      onLive: (l) => lives.push(l), onClosed: (r) => closed.push(r),
    });
    at.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64("LAST"), cursor: 10, live: false });
    await at;
    expect(lives).toEqual([false]);

    // A new session starts while the tab stays mounted: the pump's reset +
    // output arrive with no new attach. The screen must report live again so
    // the "no active agent session" chip clears.
    push(ws, init, { type: "term.reset", term_id: "agent:task-4", cursor: 10, data: b64("") });
    await tick();
    expect(lives).toEqual([false, true]);
    // Further frames do not re-report (no chip-toggling spam).
    push(ws, init, { type: "term.output", term_id: "agent:task-4", cursor: 12, data: b64("x") });
    await tick();
    expect(lives).toEqual([false, true]);

    // Session ends (screen retained) → the NEXT session's frames re-report live.
    push(ws, init, { type: "term.closed", term_id: "agent:task-4", reason: "agent_session_ended" });
    push(ws, init, { type: "term.output", term_id: "agent:task-4", cursor: 20, data: b64("y") });
    await tick();
    expect(closed).toEqual(["agent_session_ended"]);
    expect(lives).toEqual([false, true, true]);
    socket.close();
  });

  it("buffers pushes that outrun the attach response and replays them after the snapshot", async () => {
    const { socket, ws, init } = await connected();
    const order = [];
    const at = socket.attachTerminal("term-5", {
      cols: 80, rows: 24,
      onSnapshot: (b) => order.push(`snap:${dec(b)}`),
      onOutput: (b) => order.push(`out:${dec(b)}`),
    });
    at.catch(() => {});
    await tick();
    const attach = lastPayload(ws);
    // The bridge registers the sender under the AppState lock, but the response
    // is enqueued after the handler returns — a 10ms pump flush can slip in
    // ahead of it. Those bytes are PAST the snapshot cursor and must not be
    // wiped by the late-applied snapshot.
    push(ws, init, { type: "term.output", term_id: "term-5", cursor: 4, data: b64("stale") });
    push(ws, init, { type: "term.output", term_id: "term-5", cursor: 7, data: b64("tail") });
    respond(ws, init, attach.id, { snapshot: b64("SNAP"), cursor: 5 });
    await at;
    expect(order).toEqual(["snap:SNAP", "out:tail"]);
    socket.close();
  });

  it("a rejected attach deregisters the terminal instead of leaking a dead entry", async () => {
    const { socket, ws, init } = await connected();
    const outputs = [];
    const at = socket.attachTerminal("term-8", { cols: 80, rows: 24, onOutput: (b) => outputs.push(dec(b)), onSnapshot: () => {} });
    await tick();
    rejectCall(ws, init, lastPayload(ws).id, "unknown term_id");
    await expect(at).rejects.toThrow(/unknown term_id/);

    // The dead id is gone: pushes are ignored and a reconnect does not retry it.
    push(ws, init, { type: "term.output", term_id: "term-8", cursor: 3, data: b64("ghost") });
    await tick();
    expect(outputs).toEqual([]);
    socket.simulateDrop();
    await new Promise((r) => setTimeout(r, 600));
    const ws2 = FakeWebSocket.instances.at(-1);
    await handshake(ws2);
    await tick();
    const reattaches = ws2.sent
      .filter((m) => m.type === "e2ee_envelope")
      .map((m) => m.envelope.frameFields.payload.method)
      .filter((m) => m === "term.attach" || m === "agent.attach");
    expect(reattaches).toEqual([]);
    socket.close();
  });

  it("reconnect reaps an agent screen whose task is gone (no eternal retry)", async () => {
    const { socket, ws, init } = await connected();
    const closed = [];
    const at = socket.attachAgent("task-7", { cols: 120, rows: 40, onSnapshot: () => {}, onLive: () => {}, onClosed: (r) => closed.push(r) });
    at.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64("A"), cursor: 1, live: false });
    await at;

    // The task is deleted while we are away; the re-attach is rejected.
    socket.simulateDrop();
    await new Promise((r) => setTimeout(r, 600));
    const ws2 = FakeWebSocket.instances.at(-1);
    const init2 = await handshake(ws2);
    const p = lastPayload(ws2);
    expect(p.method).toBe("agent.attach");
    rejectCall(ws2, init2, p.id, "unknown task_id");
    await tick();
    expect(closed).toEqual(["reaped"]);

    // Deregistered: the next reconnect does not retry it.
    socket.simulateDrop();
    await new Promise((r) => setTimeout(r, 900));
    const ws3 = FakeWebSocket.instances.at(-1);
    await handshake(ws3);
    await tick();
    const retries = ws3.sent
      .filter((m) => m.type === "e2ee_envelope")
      .map((m) => m.envelope.frameFields.payload.method)
      .filter((m) => m === "agent.attach");
    expect(retries).toEqual([]);
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
