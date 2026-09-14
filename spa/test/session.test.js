import { describe, it, expect, vi } from "vitest";
import { DEFAULT_RPC_TIMEOUT_MS, openRelaySession, replyOrNothing } from "../src/core/session.js";
import { ApiError, selectAdapter } from "../src/core/bridgeApi/index.js";

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
  const events = { deviceKeys: [], offlineDevices: [], lost: 0 };
  const promise = openRelaySession({
    relayUrl: "wss://relay.test",
    transport: fakeTransport,
    WebSocketImpl: FakeWebSocket,
    fetchToken: async () => "tok-1",
    // The api-pinned transport keys — the fakes' relay pushes match by default.
    getPinnedDeviceKey: async (deviceId) => `pk-${deviceId}`,
    onDeviceKey: (deviceId, key) => events.deviceKeys.push([deviceId, key]),
    onDeviceOffline: (deviceId) => events.offlineDevices.push(deviceId),
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
    expect(ws.url).toBe("wss://relay.test/ws/client");
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
    const reply = session.call("run.get", { run_id: "x" });
    await tick();
    const { payload } = ws.sent.at(-1).envelope.frameFields;
    ws.serverSend({
      type: "e2ee_envelope",
      session_id: init.session_id,
      envelope: { frameFields: { payload: { id: payload.id, ok: false, error: "no such run" } } },
    });
    await expect(reply).rejects.toThrow("no such run");
  });

  it("keeps a coded refusal's wire fields on the rejection", async () => {
    const { promise, ws } = await startOpen();
    const init = await completeHandshake(ws);
    const session = await promise;
    const reply = session.call("run.get", { run_id: "x" });
    await tick();
    const { payload } = ws.sent.at(-1).envelope.frameFields;
    ws.serverSend({
      type: "e2ee_envelope",
      session_id: init.session_id,
      envelope: {
        frameFields: {
          payload: { id: payload.id, ok: false, error: "no such run", error_code: "not_found", details: { run_id: "x" } },
        },
      },
    });
    await expect(reply).rejects.toMatchObject({ error_code: "not_found", details: { run_id: "x" } });
  });

  it("takes an options object as the third argument and stamps a background priority", async () => {
    const { promise, ws } = await startOpen();
    await completeHandshake(ws);
    const session = await promise;
    session.call("git.status", { run_id: "x" }, { priority: "background" }).catch(() => {});
    session.call("board.list", {}, 5000).catch(() => {});
    session.call("run.get", {}, { timeoutMs: 5000 }).catch(() => {});
    await tick();
    const payloads = ws.sent.slice(-3).map((m) => m.envelope.frameFields.payload);
    expect(payloads[0].priority).toBe("background");
    expect(payloads[1]).not.toHaveProperty("priority");
    expect(payloads[2]).not.toHaveProperty("priority");
  });

  it("gives workspace detail no default deadline while other calls retain one", async () => {
    vi.useFakeTimers();
    try {
      FakeWebSocket.instances.length = 0;
      const promise = openRelaySession({
        relayUrl: "wss://relay.test",
        transport: fakeTransport,
        WebSocketImpl: FakeWebSocket,
        fetchToken: async () => "tok",
        getPinnedDeviceKey: async () => "pk",
      });
      await vi.advanceTimersByTimeAsync(0);
      const ws = FakeWebSocket.instances[0];
      ws.emit("open");
      await vi.advanceTimersByTimeAsync(0);
      ws.serverSend({ type: "authenticated" });
      ws.serverSend({ type: "device_key", device_id: "d", transport_public_key: "pk" });
      await vi.advanceTimersByTimeAsync(0);
      const init = ws.sent.find((message) => message.type === "session_init");
      ws.serverSend({ type: "session_accept", session_id: init.session_id, envelope: {} });
      const session = await promise;

      const workspace = session.call("workspace.get", { workspace_id: "ws-1" });
      const ordinary = session.call("project.list", {});
      workspace.catch(() => {});
      ordinary.catch(() => {});
      await vi.advanceTimersByTimeAsync(DEFAULT_RPC_TIMEOUT_MS + 1);
      await expect(ordinary).rejects.toThrow("project.list timed out");

      const request = ws.sent
        .map((message) => message.envelope?.frameFields?.payload)
        .find((payload) => payload?.method === "workspace.get");
      ws.serverSend({
        type: "e2ee_envelope",
        session_id: init.session_id,
        envelope: { frameFields: { payload: { id: request.id, ok: true, result: { id: "ws-1" } } } },
      });
      await expect(workspace).resolves.toEqual({ id: "ws-1" });
      session.close();
    } finally {
      vi.useRealTimers();
    }
  });

  it("routes calls and their refusals through the installed adapter", async () => {
    const { promise, ws } = await startOpen();
    const init = await completeHandshake(ws);
    const session = await promise;
    expect(session.adapter()).toBe(null);

    const installed = session.installAdapter(selectAdapter({ api_version: "1.1.0" }));
    expect(session.adapter()).toBe(installed);
    expect(installed.capabilities.errors.codes).toBe(true);

    const reply = session.call("run.get", { run_id: "x" });
    await tick();
    const { payload } = ws.sent.at(-1).envelope.frameFields;
    expect(payload.method).toBe("run.get");
    ws.serverSend({
      type: "e2ee_envelope",
      session_id: init.session_id,
      envelope: {
        frameFields: { payload: { id: payload.id, ok: false, error: "no such run", error_code: "not_found" } },
      },
    });
    const error = await reply.catch((thrown) => thrown);
    expect(error).toBeInstanceOf(ApiError);
    expect(error.code).toBe("not_found");
    expect(error.message).toBe("no such run");

    // A bridge nobody speaks to leaves the session with no adapter at all.
    expect(session.installAdapter({ unsupported: "app", version: "2.0.0" })).toBe(null);
    expect(session.adapter()).toBe(null);
  });

  it("hands the bridge's unsolicited pushes to onPush", async () => {
    const pushes = [];
    const { promise, ws } = await startOpen({ onPush: (payload) => pushes.push(payload) });
    const init = await completeHandshake(ws);
    await promise;
    ws.serverSend({
      type: "e2ee_envelope",
      session_id: init.session_id,
      envelope: { frameFields: { payload: { type: "entity.changed", id: "run-7" } } },
    });
    await tick();
    expect(pushes).toEqual([{ type: "entity.changed", id: "run-7" }]);
  });

  it("keeps an RPC reply out of onPush — it is somebody's answer, not a push", async () => {
    const pushes = [];
    const { promise, ws } = await startOpen({ onPush: (payload) => pushes.push(payload) });
    const init = await completeHandshake(ws);
    const session = await promise;
    const reply = session.call("ping", {});
    await tick();
    const { payload } = ws.sent.at(-1).envelope.frameFields;
    ws.serverSend({
      type: "e2ee_envelope",
      session_id: init.session_id,
      envelope: { frameFields: { payload: { id: payload.id, ok: true, result: { pong: true } } } },
    });
    await expect(reply).resolves.toEqual({ pong: true });
    expect(pushes).toEqual([]);
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
    const pending = session.call("board.list", {});
    await tick();
    ws.serverSend({ type: "device_offline", device_id: "dev-a" });
    ws.serverSend({ type: "device_offline", device_id: "dev-a" });
    await expect(pending).rejects.toMatchObject({ message: expect.stringContaining("offline"), uncertain: true });
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
    expect(events.offlineDevices).toEqual(["dev-z"]);
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
    await expect(session.call("board.list", {})).rejects.toThrow(/offline/);
  });

  it("seals the session key to the api-pinned transport key, not the relay-pushed one", async () => {
    const sealedTo = [];
    const spyTransport = {
      ...fakeTransport,
      createSessionInit: async (args) => {
        sealedTo.push(args.deviceTransportPublicKeyB64);
        return fakeTransport.createSessionInit(args);
      },
    };
    const { promise, ws } = await startOpen({
      transport: spyTransport,
      getPinnedDeviceKey: async () => "pk-dev-a",
    });
    await completeHandshake(ws, "dev-a");
    await promise;
    expect(sealedTo).toEqual(["pk-dev-a"]);
  });

  it("hard-rejects when the relay-pushed device key does not match the api-pinned key", async () => {
    const { promise, ws } = await startOpen({
      getPinnedDeviceKey: async () => "pk-genuine",
    });
    ws.serverSend({ type: "authenticated" });
    ws.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-attacker" });
    await expect(promise).rejects.toThrow(/does not match/);
    expect(ws.sent.find((m) => m.type === "session_init")).toBeUndefined();
  });

  it("refuses a device the api does not know a transport key for", async () => {
    const { promise, ws } = await startOpen({ getPinnedDeviceKey: async () => null });
    ws.serverSend({ type: "authenticated" });
    ws.serverSend({ type: "device_key", device_id: "dev-x", transport_public_key: "pk-dev-x" });
    await expect(promise).rejects.toThrow(/no pinned transport key/);
    expect(ws.sent.find((m) => m.type === "session_init")).toBeUndefined();
  });

  it("requires a pinned-key source instead of silently trusting the relay", async () => {
    await expect(
      openRelaySession({
        relayUrl: "wss://relay.test",
        transport: fakeTransport,
        WebSocketImpl: FakeWebSocket,
        fetchToken: async () => "tok",
      }),
    ).rejects.toThrow(/getPinnedDeviceKey/);
  });

  it("times out the handshake when session_accept never arrives", async () => {
    const { promise, ws } = await startOpen({ acceptTimeoutMs: 20 });
    ws.serverSend({ type: "authenticated" });
    ws.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-dev-a" });
    await tick();
    expect(ws.sent.find((m) => m.type === "session_init")).toBeDefined();
    await expect(promise).rejects.toThrow(/did not accept/);
  });

  it("fails the handshake when the target device goes offline before session_accept", async () => {
    const { promise, ws, events } = await startOpen();
    ws.serverSend({ type: "authenticated" });
    ws.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-dev-a" });
    await tick();
    ws.serverSend({ type: "device_offline", device_id: "dev-a" });
    await expect(promise).rejects.toThrow(/went offline/);
    expect(events.lost).toBe(0); // handshake failure is the caller's retry, not onLost
  });

  it("times out RPCs that never get a reply", async () => {
    vi.useFakeTimers();
    try {
      FakeWebSocket.instances.length = 0;
      const promise = openRelaySession({
        relayUrl: "wss://relay.test",
        transport: fakeTransport,
        WebSocketImpl: FakeWebSocket,
        fetchToken: async () => "tok",
        getPinnedDeviceKey: async () => "pk",
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
      // A reply that never came is not a refusal: the daemon answers a mutation
      // when its own state change is durable, and the work behind it can outlast
      // the timer. The rejection carries which of the two this is, and
      // `replyOrNothing` is the one place that reads it.
      await expect(replyOrNothing(reply)).resolves.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("says the timer is the browser's own, at twelve seconds", () => {
    expect(DEFAULT_RPC_TIMEOUT_MS).toBe(12000);
  });
});

// ---- carrying on without a reply ---------------------------------------------
// The one rule every mutation that outlives the timer follows, in the module
// that owns the timer: a reply the browser stopped waiting for is not a refusal.

describe("a reply the browser stopped waiting for", () => {
  const timedOut = (uncertain = true) =>
    Object.assign(new Error("worktree.create timed out"), { timedOut: true, ...(uncertain ? { uncertain: true } : {}) });

  it("answers nothing, so the caller carries on with what the board already has", async () => {
    await expect(replyOrNothing(Promise.reject(timedOut()))).resolves.toBeNull();
  });

  it("raises a timeout that happened before carrier handoff as a definite failure", async () => {
    await expect(replyOrNothing(Promise.reject(timedOut(false)))).rejects.toMatchObject({
      timedOut: true,
    });
  });

  it("still raises a refusal, which is the daemon saying no", async () => {
    await expect(replyOrNothing(Promise.reject(new Error("branch exists")))).rejects.toThrow("branch exists");
  });

  it("hands a reply that did arrive straight through", async () => {
    await expect(replyOrNothing(Promise.resolve({ branch: "build/x" }))).resolves.toEqual({ branch: "build/x" });
  });
});

// ---- two carriers ------------------------------------------------------------

/** A carrier a test drives: what it was handed, and what it hands back. Its
 *  listener slots are subscriptions, as the real carrier's are. */
function fakeCarrier({ sendFails = null } = {}) {
  const envelopeListeners = new Set();
  const closeListeners = new Set();
  const subscribe = (listeners) => (fn) => {
    listeners.add(fn);
    return () => listeners.delete(fn);
  };
  const carrier = {
    sent: [],
    send: async (envelope) => {
      if (sendFails) throw new Error(sendFails);
      carrier.sent.push(envelope);
    },
    onEnvelope: subscribe(envelopeListeners),
    onClose: subscribe(closeListeners),
    close: () => closeListeners.forEach((fn) => fn()),
    reply: (payload) => envelopeListeners.forEach((fn) => fn({ frameFields: { payload } })),
  };
  return carrier;
}

const replyTo = (carrier, index = -1) => carrier.sent.at(index).frameFields.payload;

describe("a session that rides two carriers", () => {
  it("sends over the peer carrier once it is riding one, and answers from it", async () => {
    const { promise, ws } = await startOpen();
    await completeHandshake(ws);
    const session = await promise;
    const peer = fakeCarrier();
    session.peer(peer);

    const reply = session.call("board.list", {});
    await tick();
    expect(peer.sent).toHaveLength(1);
    peer.reply({ id: replyTo(peer).id, ok: true, result: { tasks: [] } });
    await expect(reply).resolves.toEqual({ tasks: [] });
  });

  it("signals even while the app is paused — the pause holds user actions, not the upgrade", async () => {
    let paused = false;
    const { promise, ws } = await startOpen({ isPaused: () => paused });
    await completeHandshake(ws);
    const session = await promise;
    paused = true;
    session.call("rtc.offer", { sdp: "v=0" }).catch(() => {});
    await tick();
    expect(ws.sent.at(-1).envelope.frameFields.payload.method).toBe("rtc.offer");
    await expect(session.call("board.list", {})).rejects.toThrow(/offline/);
  });

  it("keeps signaling on the relay carrier while the peer carries", async () => {
    const { promise, ws } = await startOpen();
    await completeHandshake(ws);
    const session = await promise;
    const peer = fakeCarrier();
    session.peer(peer);

    const answered = session.call("rtc.offer", { sdp: "v=0" });
    await tick();
    const sent = ws.sent.at(-1);
    expect(sent.type).toBe("e2ee_envelope");
    expect(sent.envelope.frameFields.payload.method).toBe("rtc.offer");
    expect(peer.sent).toHaveLength(0);
    ws.serverSend({
      type: "e2ee_envelope",
      envelope: { frameFields: { payload: { id: sent.envelope.frameFields.payload.id, ok: true, result: { sdp: "v=0 a" } } } },
    });
    await expect(answered).resolves.toEqual({ sdp: "v=0 a" });
  });

  it("is not lost when the relay drops under a live peer — the device is still reachable", async () => {
    const { promise, ws, events } = await startOpen();
    await completeHandshake(ws, "dev-a");
    const session = await promise;
    session.peer(fakeCarrier());

    ws.close();
    ws.serverSend({ type: "device_offline", device_id: "dev-a" });
    await tick();
    expect(events.lost).toBe(0);
    const reply = session.call("board.list", {});
    await tick();
    expect(reply).toBeInstanceOf(Promise);
    reply.catch(() => {});
  });

  it("falls back to the relay when the channel is lost, and re-establishes there", async () => {
    const { promise, ws } = await startOpen();
    await completeHandshake(ws);
    const session = await promise;
    const carrierChanges = [];
    session.onCarrier(() => carrierChanges.push("changed"));
    const peer = fakeCarrier();
    session.peer(peer);
    expect(carrierChanges).toHaveLength(1);

    session.peer(null);
    expect(carrierChanges).toHaveLength(2);
    session.call("board.list", {}).catch(() => {});
    await tick();
    expect(ws.sent.at(-1).type).toBe("e2ee_envelope");
    expect(peer.sent).toHaveLength(0);
  });

  it("is lost once its last carrier is gone", async () => {
    const { promise, ws, events } = await startOpen();
    await completeHandshake(ws);
    const session = await promise;
    session.peer(fakeCarrier());
    ws.close();
    const pending = session.call("board.list", {});
    await tick();
    expect(events.lost).toBe(0);

    session.peer(null);
    await expect(pending).rejects.toThrow(/offline/);
    expect(events.lost).toBe(1);
    await expect(session.call("board.list", {})).rejects.toThrow(/offline/);
  });

  it("says nothing about carriers dropping after a deliberate close", async () => {
    const { promise, ws, events } = await startOpen();
    await completeHandshake(ws);
    const session = await promise;
    const peer = fakeCarrier();
    session.peer(peer);
    session.close();
    peer.close();
    ws.close();
    expect(events.lost).toBe(0);
  });

  it("hands one frame up once, however many wires it has ridden", async () => {
    const pushes = [];
    const { promise, ws } = await startOpen({ onPush: (payload) => pushes.push(payload) });
    await completeHandshake(ws);
    const session = await promise;
    session.peer(fakeCarrier());
    session.peer(null); // back on the relay carrier it started on

    ws.serverSend({ type: "e2ee_envelope", envelope: { frameFields: { payload: { type: "board.changed" } } } });
    await tick();
    expect(pushes).toEqual([{ type: "board.changed" }]);
  });

  it("fails a call whose envelope never crossed the wire, rather than waiting out its timeout", async () => {
    const { promise, ws } = await startOpen();
    await completeHandshake(ws);
    const session = await promise;
    session.peer(fakeCarrier({ sendFails: "the channel closed" }));

    await expect(session.call("board.list", {}, 60000)).rejects.toThrow(/the channel closed/);
  });

  // Spec §SPA carrier and migration policy, 6: a relay loss under a live
  // channel is not "offline". The link reconnects in the background and
  // re-presents the same session, which the bridge takes as a carrier
  // re-attach — the session id and key are minted once per session, not once
  // per socket.
  it("keeps the session over a live channel when the relay goes, and re-presents it when the relay is back", async () => {
    const { promise, ws, events } = await startOpen();
    const init = await completeHandshake(ws, "dev-a");
    const session = await promise;
    const channel = fakeCarrier();
    session.peer(channel);
    const sockets = FakeWebSocket.instances.length;

    vi.useFakeTimers();
    try {
      ws.close();
      await vi.advanceTimersByTimeAsync(0);
      expect(events.lost).toBe(0); // nothing ended: the channel is still carrying

      const working = session.call("board.list", {});
      await vi.advanceTimersByTimeAsync(0);
      channel.reply({ id: replyTo(channel).id, ok: true, result: { boards: [] } });
      await expect(working).resolves.toEqual({ boards: [] });

      // …and the relay comes back on its own, for the same session.
      await vi.advanceTimersByTimeAsync(1000);
      expect(FakeWebSocket.instances.length).toBeGreaterThan(sockets);
      const next = FakeWebSocket.instances.at(-1);
      next.emit("open");
      await vi.advanceTimersByTimeAsync(0);
      next.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-dev-a" });
      await vi.advanceTimersByTimeAsync(0);
      const again = next.sent.find((m) => m.type === "session_init");
      expect(again.session_id).toBe(init.session_id);
      expect(events.lost).toBe(0);
    } finally {
      vi.useRealTimers();
    }
    session.close();
  });

  // The whole of policy 6, from the peer connection's point of view: the ICE
  // restart it asks for while the relay is away is what the old teardown path
  // ran on, and what must now simply wait.
  it("answers an ICE restart asked for while the relay is away, once it is back", async () => {
    const { promise, ws, events } = await startOpen();
    await completeHandshake(ws, "dev-a");
    const session = await promise;
    session.peer(fakeCarrier());

    vi.useFakeTimers();
    const advance = (ms) => vi.advanceTimersByTimeAsync(ms);
    try {
      ws.close(); // the relay socket goes while the channel carries

      const restart = session.call("rtc.offer", { sdp: "v=0 restart" });
      await advance(0);

      await advance(1000); // the link's own backoff brings the relay back
      const next = FakeWebSocket.instances.at(-1);
      next.emit("open");
      await advance(0);
      next.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-dev-a" });
      await advance(0);
      const again = next.sent.find((m) => m.type === "session_init");
      next.serverSend({ type: "session_accept", session_id: again.session_id, envelope: {} });
      await advance(0);

      const offered = next.sent.at(-1);
      expect(offered.type).toBe("e2ee_envelope");
      expect(offered.envelope.frameFields.payload.method).toBe("rtc.offer");
      next.serverSend({
        type: "e2ee_envelope",
        envelope: {
          frameFields: {
            payload: { id: offered.envelope.frameFields.payload.id, ok: true, result: { sdp: "v=0 answer" } },
          },
        },
      });

      await expect(restart).resolves.toEqual({ sdp: "v=0 answer" });
      expect(events.lost).toBe(0); // App.offline never flipped
    } finally {
      vi.useRealTimers();
    }
    session.close();
  });
});
