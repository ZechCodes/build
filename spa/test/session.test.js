import { describe, it, expect, vi } from "vitest";
import { openRelaySession } from "../src/core/session.js";

// ---- fakes -------------------------------------------------------------------

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

// A transparent "transport": envelopes are plain objects, no real crypto.
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

async function startOpen(overrides = {}) {
  FakeWebSocket.instances.length = 0;
  const events = { deviceKeys: [], lost: 0 };
  const promise = openRelaySession({
    relayUrl: "ws://relay.test",
    transport: fakeTransport,
    WebSocketImpl: FakeWebSocket,
    fetchToken: async () => "tok-1",
    onDeviceKey: (deviceId, key) => events.deviceKeys.push([deviceId, key]),
    onLost: () => events.lost++,
    ...overrides,
  });
  promise.catch(() => {}); // observed via await later; avoid unhandled rejection noise
  await tick();
  const ws = FakeWebSocket.instances[0];
  ws.emit("open");
  await tick();
  return { promise, ws, events };
}

async function completeHandshake(ws, deviceId = "dev-a") {
  ws.serverSend({ type: "authenticated" });
  ws.serverSend({ type: "device_key", device_id: deviceId, transport_public_key: `pk-${deviceId}` });
  await tick();
  const init = ws.sent.find((m) => m.type === "session_init");
  ws.serverSend({ type: "session_accept", session_id: init.session_id, envelope: { ok: true } });
  await tick();
  return init;
}

// ---- tests -------------------------------------------------------------------

describe("openRelaySession", () => {
  it("authenticates first, targets the first online device, and routes RPCs", async () => {
    const { promise, ws, events } = await startOpen();
    expect(ws.url).toBe("ws://relay.test/ws/client");
    expect(ws.sent[0]).toEqual({ type: "authenticate", token: "tok-1" });

    const init = await completeHandshake(ws, "dev-a");
    expect(init.route_to).toBe("device:dev-a");
    expect(init.session_init.device_id).toBe("dev-a");

    const session = await promise;
    expect(session.deviceId).toBe("dev-a");
    expect(events.deviceKeys).toEqual([["dev-a", "pk-dev-a"]]);

    const reply = session.call("ping", {});
    await tick();
    const frame = ws.sent.at(-1);
    expect(frame.type).toBe("e2ee_envelope");
    const { payload } = frame.envelope.frameFields;
    expect(payload.method).toBe("ping");
    ws.serverSend({
      type: "e2ee_envelope",
      session_id: init.session_id,
      envelope: { frameFields: { payload: { id: payload.id, ok: true, result: { pong: true } } } },
    });
    await expect(reply).resolves.toEqual({ pong: true });
  });

  it("rejects RPC errors from the device", async () => {
    const { promise, ws } = await startOpen();
    const init = await completeHandshake(ws);
    const session = await promise;
    const reply = session.call("task.get", { task_id: "x" });
    await tick();
    const { payload } = ws.sent.at(-1).envelope.frameFields;
    ws.serverSend({
      type: "e2ee_envelope",
      session_id: init.session_id,
      envelope: { frameFields: { payload: { id: payload.id, ok: false, error: "no such task" } } },
    });
    await expect(reply).rejects.toThrow("no such task");
  });

  it("waits for the preferred device, skipping other device_key pushes", async () => {
    const { promise, ws, events } = await startOpen({ preferDeviceId: "dev-b" });
    ws.serverSend({ type: "authenticated" });
    ws.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-dev-a" });
    await tick();
    expect(ws.sent.find((m) => m.type === "session_init")).toBeUndefined();
    ws.serverSend({ type: "device_key", device_id: "dev-b", transport_public_key: "pk-dev-b" });
    await tick();
    const init = ws.sent.find((m) => m.type === "session_init");
    expect(init.route_to).toBe("device:dev-b");
    ws.serverSend({ type: "session_accept", session_id: init.session_id, envelope: {} });
    const session = await promise;
    expect(session.deviceId).toBe("dev-b");
    // Both pushes reported so the device store stays current.
    expect(events.deviceKeys).toEqual([["dev-a", "pk-dev-a"], ["dev-b", "pk-dev-b"]]);
  });

  it("fails fast when no device comes online within the wait window", async () => {
    const { promise, ws } = await startOpen({ deviceWaitMs: 20 });
    ws.serverSend({ type: "authenticated" });
    await expect(promise).rejects.toThrow("no device online");
  });

  it("fails the handshake cleanly when the socket closes early", async () => {
    const { promise, ws, events } = await startOpen();
    ws.serverSend({ type: "authenticated" });
    ws.close();
    await expect(promise).rejects.toThrow("connection closed");
    expect(events.lost).toBe(0); // handshake failure is the caller's retry, not onLost
  });

  it("signals onLost once and rejects pending calls when its device goes offline", async () => {
    const { promise, ws, events } = await startOpen();
    await completeHandshake(ws, "dev-a");
    const session = await promise;
    const pending = session.call("task.list", {});
    await tick();
    ws.serverSend({ type: "device_offline", device_id: "dev-a" });
    ws.serverSend({ type: "device_offline", device_id: "dev-a" });
    await expect(pending).rejects.toThrow("offline");
    expect(events.lost).toBe(1);
  });

  it("ignores device_offline for other devices but keeps reporting their keys", async () => {
    const { promise, ws, events } = await startOpen();
    await completeHandshake(ws, "dev-a");
    await promise;
    ws.serverSend({ type: "device_offline", device_id: "dev-z" });
    ws.serverSend({ type: "device_key", device_id: "dev-z", transport_public_key: "pk-dev-z" });
    await tick();
    expect(events.lost).toBe(0);
    expect(events.deviceKeys.at(-1)).toEqual(["dev-z", "pk-dev-z"]);
  });

  it("signals onLost when the live socket drops", async () => {
    const { promise, ws, events } = await startOpen();
    await completeHandshake(ws);
    await promise;
    ws.close();
    expect(events.lost).toBe(1);
  });

  it("close() severs the session without firing onLost", async () => {
    const { promise, ws, events } = await startOpen();
    await completeHandshake(ws);
    const session = await promise;
    session.close();
    expect(events.lost).toBe(0);
  });

  it("pauses calls while the app is offline", async () => {
    const { promise, ws } = await startOpen({ isPaused: () => true });
    await completeHandshake(ws);
    const session = await promise;
    await expect(session.call("task.list", {})).rejects.toThrow(/offline/);
  });

  it("times out RPCs that never get a reply", async () => {
    vi.useFakeTimers();
    try {
      FakeWebSocket.instances.length = 0;
      const promise = openRelaySession({
        relayUrl: "ws://relay.test",
        transport: fakeTransport,
        WebSocketImpl: FakeWebSocket,
        fetchToken: async () => "tok",
      });
      await vi.advanceTimersByTimeAsync(0);
      const ws = FakeWebSocket.instances[0];
      ws.emit("open");
      await vi.advanceTimersByTimeAsync(0);
      ws.serverSend({ type: "authenticated" });
      ws.serverSend({ type: "device_key", device_id: "d", transport_public_key: "pk" });
      await vi.advanceTimersByTimeAsync(0);
      const init = ws.sent.find((m) => m.type === "session_init");
      ws.serverSend({ type: "session_accept", session_id: init.session_id, envelope: {} });
      const session = await promise;
      const reply = session.call("slow.method", {}, 1000);
      reply.catch(() => {});
      await vi.advanceTimersByTimeAsync(1500);
      await expect(reply).rejects.toThrow("timed out");
    } finally {
      vi.useRealTimers();
    }
  });
});
