import { afterEach, beforeEach, expect, it, vi } from "vitest";

const fake = vi.hoisted(() => ({ registry: null, context: null, lock: true, cursors: new Map(), stores: [], request: null, clock: 1000 }));
vi.mock("../src/core/deviceContexts.js", () => ({
  liveContexts: () => [fake.context], captureDeviceGreeting: () => fake.context.greeting,
  onDeviceStateChanged: () => () => {},
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ sync: { device: false, projectTasks: true } }), onBridgeGreeted: () => () => {},
}));
vi.mock("../src/core/scopedSyncRegistry.js", async () => {
  const actual = await vi.importActual("../src/core/scopedSyncRegistry.js");
  return { ...actual, trackedSyncScopes: (deviceId) => fake.registry.tracked(deviceId),
    subscribeScopedSyncScopes: (listener) => fake.registry.subscribe(listener),
    subscribeScopedSyncWakes: (listener) => fake.registry.subscribeWake(listener),
  };
});
vi.mock("../src/core/localCache.js", () => ({ subscribeCache: () => () => {} }));
vi.mock("../src/core/scopedTaskBody.js", () => ({ refreshScopedTaskBody: async () => true }));
vi.mock("../src/core/scopedProjectColumns.js", () => ({ refreshScopedProjectColumns: async () => true }));
vi.mock("../src/core/scopedTaskSyncCache.js", () => ({ createScopedTaskSyncCache: (options) => {
  const key = JSON.stringify(options.scope), store = { options,
    readCursor: async () => fake.cursors.get(key), capture: async () => ({}),
    publish: async (batch) => {
      if (!options.active() || (fake.cursors.get(key)?.cursor ?? null) !== batch.expectedCursor) return false;
      fake.cursors.set(key, { cursor: batch.response.cursor, incarnation: "i", appliedHead: batch.snapshot ? batch.response.watermark : batch.response.head,
        registration: batch.registration ?? fake.cursors.get(key)?.registration });
      return true;
    }, reset: async () => { fake.cursors.delete(key); return true; },
  };
  fake.stores.push(store); return store;
} }));

const scope = { kind: "project_tasks", project_id: "p", filters: {}, order: "number" };
const entry = (owner, opened = 100, registration = owner) => ({ deviceId: "d", scope, projectId: "p",
  key: JSON.stringify(["d", scope]), id: `${owner}:1`, owner, opened, ordinal: 1, registration, registrationOpened: opened });
let runtime, channel, pending;
const queue = (owner, scopes) => { pending.push({ type: "presence", owner, scopes }); };
async function deliver(index = 0) {
  const data = pending.splice(index, 1)[0];
  await Promise.resolve(); channel.onmessage({ data });
  await runtime.captureScopedSync("d");
}
const snapshots = () => fake.request.mock.calls.filter(([, method]) => method === "sync.snapshot");
const begin = () => runtime.startScopedSyncRuntime({ ownsLock: () => fake.lock, request: fake.request });

beforeEach(async () => {
  vi.resetModules(); vi.stubGlobal("navigator", { locks: { request: vi.fn() } });
  fake.lock = true; fake.clock = 1000; fake.cursors = new Map(); fake.stores = []; pending = [];
  fake.context = { deviceId: "d", session: {}, cacheScope: {}, active: () => true,
    greeting: { epoch: "e", current: () => true, capabilities: { sync: { device: false, projectTasks: true } } } };
  channel = { onmessage: null, postMessage: vi.fn(), close: vi.fn() };
  const { createScopedSyncRegistry } = await vi.importActual("../src/core/scopedSyncRegistry.js");
  fake.registry = createScopedSyncRegistry({ channel, now: () => fake.clock });
  let snapshot = 0;
  fake.request = vi.fn(async (_deviceId, method) => method === "sync.snapshot"
    ? { status: "ok", bridge_epoch: "e", incarnation: "i", watermark: "1", cursor: `S${++snapshot}`, records: [] }
    : { status: "ok", bridge_epoch: "e", incarnation: "i", head: "1", cursor: `C${snapshot}`, changes: [] });
  runtime = await import("../src/core/scopedSyncRuntime.js"); begin();
});
afterEach(() => { runtime.stopScopedSyncRuntime(); fake.registry.dispose(); vi.unstubAllGlobals(); });

it.each(["presence first", "empty first"])("keeps a live key through reordered old X frames around Y (%s)", async (order) => {
  const x = entry("X"), y = entry("Y", 200);
  queue("X", [x]); await deliver();
  const heldStore = fake.stores[0];
  queue("X", [x]); queue("X", []);
  queue("Y", [y]); await deliver(2);
  expect(snapshots()).toHaveLength(1);
  if (order === "empty first") await deliver(1);
  await deliver();
  if (pending.length) await deliver();
  expect(snapshots()).toHaveLength(1);
  expect(fake.stores).toHaveLength(1);
  expect(heldStore.options.active()).toBe(true);
  queue("X", []); queue("Y", []); await deliver(); await deliver();
  expect(heldStore.options.active()).toBe(false);
  queue("Y", [y]); await deliver();
  expect(snapshots()).toHaveLength(2);
  expect(fake.request.mock.calls.at(-1)[2].cursor).toBe("S2");
});

it.each(["X", "Y"])("does not resnapshot continuous presence when %s closes first", async (first) => {
  queue("X", [entry("X")]); await deliver();
  queue("Y", [entry("Y", 200)]); await deliver();
  queue(first, []); await deliver();
  expect(snapshots()).toHaveLength(1);
  queue(first === "X" ? "Y" : "X", []); await deliver();
  expect(fake.stores[0].options.active()).toBe(false);
});

it.each(["presence first", "empty first"])("snapshots after an observed empty key before late X frames arrive (%s)", async (order) => {
  const x = entry("X"), y = entry("Y", 200);
  queue("X", [x]); await deliver();
  queue("X", []); await deliver();
  expect(fake.stores[0].options.active()).toBe(false);
  const before = fake.request.mock.calls.length;
  queue("X", [x]); queue("X", []); queue("Y", [y]); await deliver(2);
  expect(snapshots()).toHaveLength(2);
  expect(fake.request.mock.calls[before][1]).toBe("sync.snapshot");
  expect(fake.request.mock.calls.slice(before).some(([, , params]) => params.cursor === "C1")).toBe(false);
  if (order === "empty first") await deliver(1);
  await deliver();
  if (pending.length) await deliver();
  expect(snapshots()).toHaveLength(2);
  expect(fake.stores[1].options.active()).toBe(true);
});

it("ignores smaller opening clocks and expired legacy identities while the key remains live", async () => {
  queue("X", [entry("X", 100)]); await deliver();
  fake.clock = 5000;
  queue("Y", [entry("Y", 1, "smaller-legacy-token")]); await deliver();
  fake.clock = 91000;
  queue("Y", [entry("Y", 1, "smaller-legacy-token")]); await deliver();
  expect(fake.registry.tracked("d")).toHaveLength(1);
  expect(fake.registry.tracked("d")[0].owner).toBe("Y");
  expect(snapshots()).toHaveLength(1);
  expect(fake.stores[0].options.active()).toBe(true);
});

it.each(["holder", "reload"])("snapshots a persisted scope on every new %s engine", async (transition) => {
  queue("X", [entry("X")]); await deliver();
  const before = fake.request.mock.calls.length;
  if (transition === "holder") {
    fake.lock = false; runtime.reconcileScopedSyncRuntime(); fake.lock = true;
  } else { runtime.stopScopedSyncRuntime(); begin(); queue("X", [entry("X")]); await deliver(); }
  await runtime.captureScopedSync("d");
  expect(snapshots()).toHaveLength(2);
  const requests = fake.request.mock.calls.slice(before);
  expect(requests[0][1]).toBe("sync.snapshot");
  expect(requests.some(([, , params]) => params.cursor === "C1")).toBe(false);
});
