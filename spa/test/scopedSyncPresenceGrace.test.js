import { afterEach, beforeEach, expect, it, vi } from "vitest";

const fake = vi.hoisted(() => ({ registry: null, context: null, cursors: new Map(), stores: [], request: null }));
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
    subscribeScopedSyncWakes: (listener) => fake.registry.subscribeWake(listener) };
});
vi.mock("../src/core/localCache.js", () => ({ subscribeCache: () => () => {} }));
vi.mock("../src/core/scopedTaskBody.js", () => ({ refreshScopedTaskBody: async () => true }));
vi.mock("../src/core/scopedProjectColumns.js", () => ({ refreshScopedProjectColumns: async () => true }));
vi.mock("../src/core/scopedTaskSyncCache.js", () => ({ createScopedTaskSyncCache: (options) => {
  const key = JSON.stringify(options.scope), store = { options,
    readCursor: async () => fake.cursors.get(key), capture: async () => ({}),
    publish: async (batch) => {
      if (!options.active() || (fake.cursors.get(key)?.cursor ?? null) !== batch.expectedCursor) return false;
      fake.cursors.set(key, { cursor: batch.response.cursor, incarnation: "i",
        appliedHead: batch.snapshot ? batch.response.watermark : batch.response.head });
      return true;
    }, reset: async () => { fake.cursors.delete(key); return true; } };
  fake.stores.push(store); return store;
} }));

let runtime, channel;
const scope = { kind: "project_tasks", project_id: "p", filters: {}, order: "number" };
const remote = { deviceId: "d", projectId: "p", scope, key: JSON.stringify(["d", scope]),
  owner: "hidden-follower", id: "hidden-follower:1", opened: 0, ordinal: 1 };
const presence = (scopes = [remote]) => channel.onmessage({ data: { type: "presence", owner: remote.owner, scopes } });
const snapshots = () => fake.request.mock.calls.filter(([, method]) => method === "sync.snapshot");

beforeEach(async () => {
  vi.useFakeTimers(); vi.setSystemTime(0); vi.resetModules();
  vi.stubGlobal("navigator", { locks: { request: vi.fn() } });
  fake.cursors = new Map(); fake.stores = [];
  fake.context = { deviceId: "d", session: {}, cacheScope: {}, active: () => true,
    greeting: { epoch: "e", current: () => true, capabilities: { sync: { device: false, projectTasks: true } } } };
  channel = { onmessage: null, postMessage: vi.fn(), close: vi.fn() };
  const { createScopedSyncRegistry } = await vi.importActual("../src/core/scopedSyncRegistry.js");
  fake.registry = createScopedSyncRegistry({ channel });
  let snapshot = 0;
  fake.request = vi.fn(async (_deviceId, method) => {
    if (method === "sync.snapshot") return { status: "ok", bridge_epoch: "e", incarnation: "i", watermark: "1", cursor: `S${++snapshot}`, records: [] };
    return { status: "ok", bridge_epoch: "e", incarnation: "i", head: "1", cursor: `C${snapshot}`, changes: [] };
  });
  runtime = await import("../src/core/scopedSyncRuntime.js");
  runtime.startScopedSyncRuntime({ ownsLock: () => true, request: fake.request });
});
afterEach(() => { runtime?.stopScopedSyncRuntime(); fake.registry?.dispose(); vi.useRealTimers(); vi.unstubAllGlobals(); });

it("keeps a hidden follower continuously tracked across sixty-second heartbeats and releases it immediately", async () => {
  presence(); await runtime.captureScopedSync("d");
  expect(snapshots()).toHaveLength(1);
  const original = fake.stores[0];
  for (let heartbeat = 0; heartbeat < 3; heartbeat += 1) {
    await vi.advanceTimersByTimeAsync(59_999);
    expect(fake.registry.tracked("d")).toHaveLength(1);
    expect(original.options.active()).toBe(true);
    await vi.advanceTimersByTimeAsync(1);
    presence(); await runtime.captureScopedSync("d");
    expect(snapshots()).toHaveLength(1);
    expect(fake.stores).toHaveLength(1);
  }
  presence([]);
  expect(fake.registry.tracked("d")).toEqual([]);
  expect(original.options.active()).toBe(false);
  expect(snapshots()).toHaveLength(1);
});

it("keeps remote grace-period scopes inside the same sixteen-scope cap", async () => {
  presence();
  const { taskSyncScope } = await vi.importActual("../src/core/scopedSyncRegistry.js");
  for (let index = 0; index < 15; index += 1) fake.registry.register({ deviceId: "d", projectId: "p", scope: taskSyncScope(`other-${index}`) });
  await vi.advanceTimersByTimeAsync(60_000);
  const tracked = fake.registry.tracked("d");
  expect(tracked).toHaveLength(16);
  expect(tracked.some((entry) => entry.owner === remote.owner)).toBe(true);
});
