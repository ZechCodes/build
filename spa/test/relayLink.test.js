// One relay socket, end to end: the handshake that mints a session, the carrier
// it hands over, and the backoff reconnect that re-presents the same session
// while something else is carrying it.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createRelayLink } from "../src/core/relayLink.js";

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
  find(type) {
    return this.sent.find((m) => m.type === type);
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

// The backoff is a clock, so every test here runs on one it controls: on real
// timers a loaded machine overshoots the wait and the assertions land on
// whichever attempt the overshoot reached.
beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const settle = () => vi.advanceTimersByTimeAsync(0);
/** Past the longest backoff a lost socket waits out. */
const backoff = () => vi.advanceTimersByTimeAsync(LONGEST_RETRY_MS + 1);

function linkOn(overrides = {}) {
  FakeWebSocket.instances.length = 0;
  const seen = { sessions: [], carriers: [], connecting: 0, deviceKeys: [], offline: [] };
  const link = createRelayLink({
    relayUrl: "wss://relay.test",
    transport: fakeTransport,
    WebSocketImpl: FakeWebSocket,
    fetchToken: async () => "tok-1",
    getPinnedDeviceKey: async (deviceId) => `pk-${deviceId}`,
    preferDeviceId: () => null,
    onConnecting: () => seen.connecting++,
    onDeviceKey: (id, key) => seen.deviceKeys.push([id, key]),
    onDeviceOffline: (id) => seen.offline.push(id),
    onSession: (session) => seen.sessions.push(session),
    onRelay: (carrier) => seen.carriers.push(carrier),
    ...overrides,
  });
  return { link, seen };
}

/** Answer the relay's half of one socket's handshake. */
async function greet(ws, deviceId = "dev-a") {
  ws.emit("open");
  await settle();
  ws.serverSend({ type: "authenticated" });
  ws.serverSend({ type: "device_key", device_id: deviceId, transport_public_key: `pk-${deviceId}` });
  await settle();
  const init = ws.find("session_init");
  ws.serverSend({ type: "session_accept", session_id: init.session_id, envelope: {} });
  await settle();
  return init;
}

/** A link that is up, and the socket it came up on. */
async function connected(overrides = {}) {
  const { link, seen } = linkOn(overrides);
  const started = link.start();
  started.catch(() => {});
  await settle();
  const ws = FakeWebSocket.instances.at(-1);
  const init = await greet(ws);
  await started;
  return { link, seen, ws, init };
}

/** How long the link will ever wait before trying again. Its mirror is
 *  LONGEST_RETRY_MS in relayLink.js. */
const LONGEST_RETRY_MS = 8000;

/** The next socket the link opens on its own, once its backoff has run. */
async function nextSocket(after) {
  await backoff();
  expect(FakeWebSocket.instances.length).toBeGreaterThan(after);
  return FakeWebSocket.instances.at(-1);
}

describe("createRelayLink", () => {
  it("authenticates, seals to the pinned key, and hands the carrier over", async () => {
    const { link, seen, ws, init } = await connected();

    expect(ws.url).toBe("wss://relay.test/ws/client");
    expect(ws.sent[0]).toEqual({ type: "authenticate", token: "tok-1" });
    expect(init.route_to).toBe("device:dev-a");
    expect(seen.sessions).toEqual([{ sessionId: init.session_id, sessionKeyB64: `key-${init.session_id}`, deviceId: "dev-a" }]);
    expect(seen.carriers).toHaveLength(1);
    expect(link.deviceId()).toBe("dev-a");
    link.close();
  });

  it("refuses a relay-supplied device key that is not the api-pinned one", async () => {
    const { link } = linkOn();
    const started = link.start();
    started.catch(() => {});
    await settle();
    const ws = FakeWebSocket.instances.at(-1);
    ws.emit("open");
    await settle();
    ws.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-somebody-else" });

    await expect(started).rejects.toThrow(/does not match the api-pinned key/);
    link.close();
  });

  it("hands back the relay slot when the socket goes, and reconnects on its own", async () => {
    const { link, seen, ws } = await connected();
    const sockets = FakeWebSocket.instances.length;

    ws.close();
    await settle();
    expect(seen.carriers.at(-1)).toBe(null);

    const next = await nextSocket(sockets);
    expect(next).not.toBe(ws);
    await greet(next);
    expect(seen.carriers.at(-1)).not.toBe(null);
    link.close();
  });

  it("re-presents the same session while something else is carrying it", async () => {
    const carried = { carrying: () => ({ a: "channel" }) };
    const { link, seen, ws, init } = await connected(carried);
    const sockets = FakeWebSocket.instances.length;

    ws.close();
    const next = await nextSocket(sockets);
    const again = await greet(next);

    expect(again.session_id).toBe(init.session_id);
    expect(seen.sessions).toHaveLength(1); // the same session, a second carrier
    link.close();
  });

  it("mints a new session when nothing is carrying the old one", async () => {
    const { link, seen, ws, init } = await connected();
    const sockets = FakeWebSocket.instances.length;

    ws.close();
    const again = await greet(await nextSocket(sockets));

    expect(again.session_id).not.toBe(init.session_id);
    expect(seen.sessions).toHaveLength(2);
    link.close();
  });

  it("gives up on a session the device would not take back, rather than presenting it forever", async () => {
    const carried = { carrying: () => ({ a: "channel" }), acceptTimeoutMs: 20 };
    const { link, seen, ws, init } = await connected(carried);
    const sockets = FakeWebSocket.instances.length;

    // The device holds this session under another key: the re-attach it refuses
    // is never answered.
    ws.close();
    const refusing = await nextSocket(sockets);
    refusing.emit("open");
    await settle();
    refusing.serverSend({ type: "device_key", device_id: "dev-a", transport_public_key: "pk-dev-a" });
    await settle();
    expect(refusing.find("session_init").session_id).toBe(init.session_id);

    await vi.advanceTimersByTimeAsync(50); // the accept it will never get
    const afterRefusal = await nextSocket(FakeWebSocket.instances.length);
    const fresh = await greet(afterRefusal);
    expect(fresh.session_id).not.toBe(init.session_id);
    expect(seen.sessions).toHaveLength(2);
    link.close();
  });

  it("says its device dropped off the relay by handing the slot back", async () => {
    const { link, seen, ws } = await connected();

    ws.serverSend({ type: "device_offline", device_id: "dev-a" });
    await settle();

    expect(seen.offline).toEqual(["dev-a"]);
    expect(seen.carriers.at(-1)).toBe(null);
    link.close();
  });

  it("stops trying once it is closed", async () => {
    const { link, ws } = await connected();
    const sockets = FakeWebSocket.instances.length;

    link.close();
    ws.close();
    await backoff();

    expect(FakeWebSocket.instances).toHaveLength(sockets);
  });

  it("reports every device the relay knows about, whoever the session is with", async () => {
    const { link, seen, ws } = await connected();

    ws.serverSend({ type: "device_key", device_id: "dev-b", transport_public_key: "pk-dev-b" });
    await settle();

    expect(seen.deviceKeys).toContainEqual(["dev-b", "pk-dev-b"]);
    link.close();
  });
});
