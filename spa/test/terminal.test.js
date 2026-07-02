import { describe, it, expect } from "vitest";
import { TerminalSession } from "../src/terminal/session.js";

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

describe("TerminalSession", () => {
  it("skips the authenticated frame and other devices, attaches to the preferred one", async () => {
    FakeWebSocket.instances.length = 0;
    const session = new TerminalSession({
      url: "ws://relay.test",
      transport: fakeTransport,
      WebSocketImpl: FakeWebSocket,
      getToken: async () => "tok-9",
      getPinnedDeviceKey: async (deviceId) => `pk-${deviceId.slice(-1)}`,
      preferDeviceId: () => "dev-b",
    });
    const outputs = [];
    const snapshots = [];
    session.onOutput((bytes) => outputs.push(new TextDecoder().decode(bytes)));
    session.onSnapshot((bytes) => snapshots.push(new TextDecoder().decode(bytes)));

    const started = session.start(120, 20);
    started.catch(() => {});
    await tick();
    const ws = FakeWebSocket.instances[0];
    ws.emit("open");
    await tick();
    expect(ws.sent[0]).toEqual({ type: "authenticate", token: "tok-9" });

    ws.serverSend({ type: "authenticated" });
    ws.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-a" });
    ws.serverSend({ type: "device_key", device_id: "dev-b", transport_public_key: "pk-b" });
    await tick();
    const init = ws.sent.find((m) => m.type === "session_init");
    expect(init.route_to).toBe("device:dev-b");
    ws.serverSend({ type: "session_accept", session_id: init.session_id, envelope: {} });
    await tick();

    // term.attach → snapshot applied, cursor recorded.
    const attach = ws.sent.at(-1).envelope.frameFields.payload;
    expect(attach.method).toBe("term.attach");
    expect(attach.params).toEqual({ cols: 120, rows: 20 });
    ws.serverSend({
      type: "e2ee_envelope",
      session_id: init.session_id,
      envelope: { frameFields: { payload: { id: attach.id, ok: true, result: { snapshot: b64("SCREEN"), cursor: 5 } } } },
    });
    await started;
    expect(snapshots).toEqual(["SCREEN"]);

    // Live output flows, but stale cursors (replays) are dropped.
    const push = (cursor, data) =>
      ws.serverSend({
        type: "e2ee_envelope",
        session_id: init.session_id,
        envelope: { frameFields: { payload: { type: "term.output", cursor, data: b64(data) } } },
      });
    push(6, "live");
    push(4, "stale");
    await tick();
    expect(outputs).toEqual(["live"]);
    session.close();
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
    const session = new TerminalSession({
      url: "ws://relay.test",
      transport: spyTransport,
      WebSocketImpl: FakeWebSocket,
      getToken: async () => "tok",
      getPinnedDeviceKey: async () => "pk-genuine",
      preferDeviceId: () => "dev-a",
    });
    const started = session.start(80, 24);
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
    session.close();
  });
});
