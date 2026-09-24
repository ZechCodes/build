// @vitest-environment jsdom
// #120 against the bridge itself: a message this tab already holds changes
// while the tab is disconnected, and the reconnect repairs it in the cache.
//
// Nothing on the wire or in the sync layer is a stand-in. The bridge is the
// real `AppState` behind real sessions (bridge/examples/session_bridge.rs):
// real `session.hello`, real `changes.subscribe` pushes, real `thread.page`
// paging, a real delivery queue deciding when the message is sent. The SPA
// side is the real greeting (core/changeEvents.js), the real ordered pass
// (core/cacheSync.js), the real thread read (core/threadSync.js) and the real
// cache over fake-indexeddb. Only the device registry and the route are held
// here — which session this device is on is what a reconnect changes.

import { spawn } from "node:child_process";
import { resolve } from "node:path";
import readline from "node:readline";
import { afterAll, beforeAll, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/app.js", () => ({ App }));

const contexts = new Map();
const stateListeners = new Set();
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: (deviceId) => contexts.get(deviceId) || null,
  liveContexts: () => [...contexts.values()],
  onDeviceStateChanged: (fn) => {
    stateListeners.add(fn);
    return () => stateListeners.delete(fn);
  },
}));

const cache = await import("../src/core/localCache.js");
const changeEvents = await import("../src/core/changeEvents.js");
const sync = await import("../src/core/cacheSync.js");

const DEVICE = "dev-1";
const bridgeRoot = resolve(process.cwd(), "../bridge");

/** The bridge process and the one JSON line protocol it speaks. */
function startBridge() {
  const child = spawn("cargo", ["run", "-q", "--example", "session_bridge"], {
    cwd: bridgeRoot,
    stdio: ["pipe", "pipe", "pipe"],
  });
  // What cargo and the bridge said, kept for the failure message only: the
  // bridge logs every lifecycle event, and a passing run has no use for them.
  let said = "";
  child.stderr.on("data", (chunk) => { said = `${said}${chunk}`.slice(-4000); });
  const heard = [];
  const waiters = new Set();
  readline.createInterface({ input: child.stdout }).on("line", (text) => {
    const line = JSON.parse(text);
    if (line.session && line.frame?.type === "changes") {
      // A push, at whichever session is this device's now.
      if (contexts.get(DEVICE)?.session.id === line.session) changeEvents.dispatchChangeEvent(line.frame, DEVICE);
      return;
    }
    const waiter = [...waiters].find((candidate) => candidate.match(line));
    if (!waiter) return void heard.push(line);
    waiters.delete(waiter);
    waiter.resolve(line);
  });
  const next = (match, waitMs = 20000) => {
    const early = heard.findIndex(match);
    if (early >= 0) return Promise.resolve(heard.splice(early, 1)[0]);
    return new Promise((done, fail) => {
      const waiter = { match, resolve: done };
      waiters.add(waiter);
      setTimeout(() => waiters.delete(waiter) && fail(new Error(`the bridge did not answer:\n${said}`)), waitMs).unref();
    });
  };
  const send = (line) => child.stdin.write(`${JSON.stringify(line)}\n`);
  let ids = 0;
  const answered = (reply, method) => {
    if (!reply.ok) throw new Error(`${method}: ${reply.error}`);
    return reply.result;
  };
  return {
    child,
    next,
    /** A call from no connected client: another device, or the agent's world. */
    direct: async (method, params) => {
      const id = ++ids;
      send({ op: "direct", id, method, params });
      return answered((await next((line) => line.direct && line.id === id)).reply, method);
    },
    /** A call on one session, the way this tab's rpc sends it. */
    call: async (session, method, params) => {
      const id = ++ids;
      send({ op: "call", session, id, method, params });
      return answered((await next((line) => line.session === session && line.frame?.id === id && "ok" in line.frame)).frame, method);
    },
    open: async (session) => {
      send({ op: "open", session });
      await next((line) => line.session === session && line.opened);
    },
    close: async (session) => {
      send({ op: "close", session });
      await next((line) => line.session === session && line.closed);
    },
  };
}

let bridge;
let ready;

beforeAll(async () => {
  bridge = startBridge();
  ready = await bridge.next((line) => line.ready, 600_000);
}, 610_000);

afterAll(async () => {
  sync.stopCacheSync();
  changeEvents.resetChangeEvents();
  bridge?.child.stdin.end();
  bridge?.child.kill();
});

/** This device on one session: registered, greeted over it, and announced. */
async function connectOn(session) {
  await bridge.open(session);
  const call = (method, params) => bridge.call(session, method, params);
  const context = {
    deviceId: DEVICE,
    rpc: call,
    session: { id: session },
    cacheScope: { deviceId: DEVICE, active: () => true },
    active: () => contexts.get(DEVICE) === context,
  };
  context.greeted = changeEvents.greetBridge(call, { deviceId: DEVICE, isCurrent: () => contexts.get(DEVICE) === context });
  contexts.set(DEVICE, context);
  stateListeners.forEach((listener) => listener());
  await context.greeted;
}

/** The connection drops: the bridge ends the session, and this tab has none. */
async function disconnect(session) {
  contexts.delete(DEVICE);
  await bridge.close(session);
}

/** The message a post named, as this tab's cache holds it. */
async function cachedMessage(operationId) {
  for (const sub of await cache.cachedSubKeys(DEVICE, ready.run_id, "thread")) {
    const record = await cache.readCached({ deviceId: DEVICE, entityId: ready.run_id, kind: "thread", sub });
    const found = record?.value?.items?.find((item) => item.data?.operation_id === operationId);
    if (found) return found;
  }
  return null;
}

/** Waits for the cache to say something, or for the time to run out. */
async function eventually(read, holds, waitMs = 20000) {
  const until = Date.now() + waitMs;
  let seen = await read();
  while (!holds(seen) && Date.now() < until) {
    await new Promise((done) => setTimeout(done, 50));
    seen = await read();
  }
  return seen;
}

it("repairs a loaded message whose delivery changed while the device was away", async () => {
  const { run_id: entityId, agent_id: agentId, conversation_id: conversationId } = ready;
  // The running session was started on another model, so a message to it
  // waits in the queue until a turn that may replace the session comes.
  await bridge.direct("agent.choose", { entity_id: entityId, agent_id: agentId, conversation_id: conversationId, model: "claude-opus-5", effort: "high" });
  await bridge.direct("thread.post", { entity_id: entityId, agent_id: agentId, body: "B: wait for the next turn", operation_id: "op-b" });

  sync.startCacheSync();
  await connectOn("s1");
  const loaded = await eventually(() => cachedMessage("op-b"), Boolean);
  expect(loaded?.data.delivery_status).toBe("queued");

  await disconnect("s1");
  // Away: another device's interrupting post replaces the session, and the
  // queued message is delivered — in place, under the new tail.
  await bridge.direct("thread.post", { entity_id: entityId, agent_id: agentId, body: "C: go now", operation_id: "op-c", interrupt: true });
  const onBridge = await eventually(
    async () => (await bridge.direct("thread.page", { entity_id: entityId, agent_id: agentId, limit: 50 })).items,
    (items) => items.find((item) => item.data.operation_id === "op-b")?.data.delivery_status === "sent",
  );
  const delivered = onBridge.find((item) => item.data.operation_id === "op-b");
  expect(delivered.data.delivery_status).toBe("sent");
  expect(Math.max(...onBridge.map((item) => item.data.sequence))).toBeGreaterThan(delivered.data.sequence);
  expect(await cachedMessage("op-b")).toEqual(loaded); // nothing reached this tab while it was away

  await connectOn("s2");
  const repaired = await eventually(() => cachedMessage("op-b"), (item) => item?.data.delivery_status === "sent");
  expect(repaired.data.delivery_status).toBe("sent");
  expect(repaired.data.updated_sequence).toBe(delivered.data.updated_sequence);
  expect(await cachedMessage("op-c")).toBeTruthy();
}, 60_000);
