import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const fake = vi.hoisted(() => ({ context: null, lock: true, active: true, answering: true, greeted: true, opening: null,
  epoch: "e1", head: 1, snapshot: 0, origins: new Map(), scopes: [], stores: [], createStore: null, changed: null, request: null, respond: null }));
vi.mock("../src/core/deviceContexts.js", async () => ({
  ...await vi.importActual("../src/core/deviceContexts.js"),
  liveContexts: () => fake.answering ? [fake.context] : [], onDeviceStateChanged: () => () => {},
  captureDeviceGreeting: (context) => {
    if (!fake.greeted || !context.session) return null;
    const opening = fake.opening, session = context.session, epoch = fake.epoch;
    return { epoch, capabilities: { sync: { projectTasks: true } },
      current: () => fake.greeted && fake.opening === opening && context.session === session && fake.epoch === epoch };
  },
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ sync: { projectTasks: true } }), onBridgeGreeted: () => () => {},
}));
vi.mock("../src/core/scopedSyncRegistry.js", async () => ({
  ...await vi.importActual("../src/core/scopedSyncRegistry.js"),
  trackedSyncScopes: () => fake.scopes, subscribeScopedSyncScopes: (listener) => { fake.changed = listener; return () => {}; },
  subscribeScopedSyncWakes: () => () => {},
}));
vi.mock("../src/core/scopedTaskSyncCache.js", async () => {
  const actual = await vi.importActual("../src/core/scopedTaskSyncCache.js");
  return { ...actual, createScopedTaskSyncCache: (options) => {
    const store = fake.createStore(options); fake.stores.push(store); return store;
  } };
});
vi.mock("../src/core/scopedProjectColumns.js", () => ({ refreshScopedProjectColumns: async () => true }));
vi.mock("../src/core/scopedTaskBody.js", () => ({ refreshScopedTaskBody: async () => true }));

const deviceId = "reconnect-device", projectId = "proj-1";
const scope = { kind: "project_tasks", project_id: projectId, filters: { states: ["open"] }, order: "number" };
const entry = { deviceId, projectId, scope, key: JSON.stringify([deviceId, scope]), id: "mounted-view" };
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
const row = (number, change = false) => ({ kind: "task", key: `task-${number}`, revision: String(number), member: true,
  ...(change ? { seq: String(number), operation: "upsert" } : {}),
  row: { id: `task-${number}`, project_id: projectId, number, title: `Task ${number}`, state: "open", watched: false,
    updated_at: `2026-11-03T12:00:0${number}Z` } });
let runtime, local, queryAddress, cursorAddress;
const createContext = () => {
  const context = { deviceId, session: {}, cacheScope: {}, active: () => fake.active && fake.context === context };
  return context;
};
const begin = () => runtime.startScopedSyncRuntime({ ownsLock: () => fake.lock, request: fake.request });
const capture = () => runtime.captureScopedSync(fake.context.deviceId);
const snapshots = () => fake.request.mock.calls.filter(([, method]) => method === "sync.snapshot");
const ids = async () => (await local.readCached(queryAddress))?.value?.ids;
function nextOpening({ sameSession = false, sameEpoch = false } = {}) {
  if (!sameSession) fake.context.session = {};
  fake.opening = {};
  if (!sameEpoch) fake.epoch = "e2";
  fake.greeted = true;
  runtime.reconcileScopedSyncRuntime();
}

beforeEach(async () => {
  vi.resetModules(); vi.stubGlobal("navigator", { locks: { request: vi.fn() } });
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  Object.assign(fake, { lock: true, active: true, answering: true, greeted: true, opening: {}, epoch: "e1", head: 1,
    snapshot: 0, origins: new Map(), scopes: [entry], stores: [] });
  fake.context = createContext();
  fake.respond = (method, params) => {
    const wire = { status: "ok", incarnation: "i", bridge_epoch: fake.epoch };
    if (method === "sync.snapshot") {
      const cursor = `S${++fake.snapshot}`; fake.origins.set(cursor, fake.head);
      return { ...wire, cursor, watermark: String(fake.head), records: Array.from({ length: fake.head }, (_, index) => row(index + 1)) };
    }
    if (method === "sync.head") return { ...wire, head: String(fake.head), retention_floor: "0" };
    const from = fake.origins.get(params.cursor);
    if (from === undefined) throw new Error("Unexpected cursor origin");
    const cursor = `C${fake.head}`; fake.origins.set(cursor, fake.head);
    return { ...wire, cursor, head: String(fake.head), changes: Array.from({ length: fake.head - from }, (_, index) => row(from + index + 1, true)) };
  };
  fake.request = vi.fn(async (_deviceId, method, params) => fake.respond(method, params));
  local = await import("../src/core/localCache.js");
  fake.createStore = (await vi.importActual("../src/core/scopedTaskSyncCache.js")).createScopedTaskSyncCache;
  const { summaryQueryAddress } = await import("../src/core/taskSummaryCache.js");
  const { syncCursorAddress } = await import("../src/core/scopedTaskSyncCache.js");
  queryAddress = summaryQueryAddress({ deviceId, projectId }, scope.filters);
  cursorAddress = syncCursorAddress(deviceId, scope, projectId);
  runtime = await import("../src/core/scopedSyncRuntime.js"); begin(); await capture();
  expect((await local.readCached(cursorAddress)).value.cursor).toBe("C1");
});
afterEach(() => { runtime?.stopScopedSyncRuntime(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("continues the holder's committed cursor through a greeting gap and session reconnect", async () => {
  fake.answering = false; fake.greeted = false; fake.context.session = null; runtime.reconcileScopedSyncRuntime();
  fake.answering = true; fake.head = 2; nextOpening(); await capture();
  expect(snapshots()).toHaveLength(1);
  expect(fake.stores).toHaveLength(1);
  expect(fake.request.mock.calls.at(-1)).toMatchObject([deviceId, "sync.changes", { cursor: "C1" }, scope]);
  expect(await ids()).toEqual(["task-2", "task-1"]);
});

it.each(["session", "same-session hello"])("detaches an old held response when the %s opening changes", async (kind) => {
  fake.head = 2;
  const staleAnswer = fake.respond("sync.changes", { cursor: "C1" }), held = deferred();
  fake.request.mockImplementationOnce(() => held.promise);
  const old = runtime.wakeScopedSync(deviceId);
  await vi.waitFor(() => expect(fake.request).toHaveBeenCalledTimes(3));
  fake.head = 3; nextOpening({ sameSession: kind === "same-session hello", sameEpoch: kind === "same-session hello" });
  const current = capture();
  await vi.waitFor(() => expect(fake.request).toHaveBeenCalledTimes(4));
  await current;
  expect(snapshots()).toHaveLength(1);
  expect((await local.readCached(cursorAddress)).value.cursor).toBe("C3");
  held.resolve(staleAnswer); await old;
  expect((await local.readCached(cursorAddress)).value.cursor).toBe("C3");
  expect(await ids()).toEqual(["task-3", "task-2", "task-1"]);
});

it("discards a capture that resumes after its request opening ended", async () => {
  const store = fake.stores[0], read = store.capture, held = deferred();
  let waiting = false;
  vi.spyOn(store, "capture").mockImplementationOnce(async () => { const result = await read(); waiting = true; await held.promise; return result; });
  const old = runtime.wakeScopedSync(deviceId);
  await vi.waitFor(() => expect(waiting).toBe(true));
  fake.head = 2; nextOpening(); await capture();
  const before = fake.request.mock.calls.length;
  held.resolve(); await old;
  expect(fake.request.mock.calls).toHaveLength(before);
  expect(snapshots()).toHaveLength(1);
  expect((await local.readCached(cursorAddress)).value.cursor).toBe("C2");
});

it("fences an old publication inside its transaction while the new reply is still held", async () => {
  const store = fake.stores[0], publish = store.publish, publication = deferred();
  let waiting = false;
  vi.spyOn(store, "publish").mockImplementationOnce(async (batch) => { waiting = true; await publication.promise; return publish(batch); });
  fake.head = 2;
  const old = runtime.wakeScopedSync(deviceId);
  await vi.waitFor(() => expect(waiting).toBe(true));
  fake.head = 3;
  const reply = fake.respond("sync.changes", { cursor: "C1" }), held = deferred();
  fake.request.mockImplementationOnce(() => held.promise);
  nextOpening();
  const current = capture();
  await vi.waitFor(() => expect(fake.request).toHaveBeenCalledTimes(4));
  publication.resolve(); await old;
  expect((await local.readCached(cursorAddress)).value.cursor).toBe("C1");
  held.resolve({ ...reply, bridge_epoch: "e2" }); await current;
  expect(snapshots()).toHaveLength(1);
  expect((await local.readCached(cursorAddress)).value.cursor).toBe("C3");
});

it("fences an old cursor reset inside its transaction while the new reply is held", async () => {
  const store = fake.stores[0], reset = store.reset, removal = deferred();
  let waiting = false;
  vi.spyOn(store, "reset").mockImplementationOnce(async (options) => { waiting = true; await removal.promise; return reset(options); });
  fake.request.mockImplementationOnce(async () => ({ status: "resync_required", reason: "retention_expired", bridge_epoch: "e1" }));
  const old = runtime.wakeScopedSync(deviceId);
  await vi.waitFor(() => expect(waiting).toBe(true));
  fake.head = 2;
  const reply = fake.respond("sync.changes", { cursor: "C1" }), held = deferred();
  fake.request.mockImplementationOnce(() => held.promise);
  nextOpening(); const current = capture();
  await vi.waitFor(() => expect(fake.request).toHaveBeenCalledTimes(4));
  removal.resolve(); await old;
  expect((await local.readCached(cursorAddress))?.value.cursor).toBe("C1");
  held.resolve({ ...reply, bridge_epoch: "e2" }); await current;
  expect(snapshots()).toHaveLength(1);
  expect((await local.readCached(cursorAddress)).value.cursor).toBe("C2");
});

it("snapshots when a reconnect's cursor was replaced outside this engine", async () => {
  const saved = await local.readCached(cursorAddress);
  await local.writeCached(cursorAddress, { ...saved.value, cursor: "external-cursor" });
  const before = fake.request.mock.calls.length;
  fake.head = 2; nextOpening(); await capture();
  expect(fake.request.mock.calls[before][1]).toBe("sync.snapshot");
  expect(fake.request.mock.calls.slice(before).some(([, , params]) => params.cursor === "external-cursor")).toBe(false);
  expect(snapshots()).toHaveLength(2);
});

it("retains a committed changes cursor when the opening ends before publication returns", async () => {
  const store = fake.stores[0], publish = store.publish;
  vi.spyOn(store, "publish").mockImplementationOnce(async (batch) => {
    const committed = await publish(batch);
    expect(committed).toBe(true);
    // IndexedDB has committed C2, but the old request opening is gone before
    // the engine receives that successful result.
    fake.greeted = false; fake.context.session = null;
    return committed;
  });
  fake.head = 2;
  await runtime.wakeScopedSync(deviceId);
  expect((await local.readCached(cursorAddress)).value.cursor).toBe("C2");
  const before = fake.request.mock.calls.length;
  fake.head = 3; nextOpening(); await capture();
  expect(fake.request.mock.calls.slice(before).map(([, method]) => method)).toEqual(["sync.changes"]);
  expect(fake.request.mock.calls[before][2].cursor).toBe("C2");
  expect(snapshots()).toHaveLength(1);
  expect(await ids()).toEqual(["task-3", "task-2", "task-1"]);
});

it("obeys bridge resync_required after a reconnect", async () => {
  fake.request.mockImplementationOnce(async () => ({ status: "resync_required", reason: "retention_expired", bridge_epoch: "e2" }));
  const before = fake.request.mock.calls.length;
  fake.head = 2; nextOpening(); await capture();
  expect(fake.request.mock.calls.slice(before).map(([, method]) => method)).toEqual(["sync.changes", "sync.snapshot", "sync.changes"]);
  expect(snapshots()).toHaveLength(2);
  expect(await ids()).toEqual(["task-2", "task-1"]);
});

it.each(["lock", "cache scope", "context", "identity", "reload", "key gap"])("still snapshots after a true %s lifetime boundary", async (kind) => {
  fake.head = 2;
  if (kind === "lock") { fake.lock = false; runtime.reconcileScopedSyncRuntime(); fake.lock = true; }
  if (kind === "cache scope") fake.context.cacheScope = {};
  if (kind === "context") fake.context = createContext();
  if (kind === "identity") fake.context.deviceId = "different-bridge";
  if (kind === "reload") { runtime.stopScopedSyncRuntime(); begin(); }
  if (kind === "key gap") { fake.scopes = []; fake.changed(); fake.scopes = [entry]; fake.changed(); }
  runtime.reconcileScopedSyncRuntime(); await capture();
  expect(snapshots()).toHaveLength(2);
});
