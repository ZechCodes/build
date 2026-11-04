import { beforeEach, describe, expect, it, vi } from "vitest";
const fake = vi.hoisted(() => ({ contexts: [], scopes: [], lock: false, engines: [], scopeChanged: null,
  woken: null, broadcast: vi.fn(), changed: null, greeted: null, cacheChanged: null, columns: vi.fn(async () => true), body: vi.fn(async () => true) }));
vi.mock("../src/core/deviceContexts.js", () => ({
  liveContexts: () => fake.contexts,
  captureDeviceGreeting: (context) => context.greeting,
  onDeviceStateChanged: (fn) => { fake.changed = fn; return () => {}; },
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ sync: { projectTasks: true, task: true } }),
  onBridgeGreeted: (fn) => { fake.greeted = fn; return () => {}; },
}));
vi.mock("../src/core/scopedSyncRegistry.js", () => ({
  trackedSyncScopes: () => fake.scopes,
  scopeIsTracked: (_deviceId, scope) => fake.scopes.some((entry) => JSON.stringify(entry.scope) === JSON.stringify(scope)),
  broadcastScopedSyncWake: (...args) => fake.broadcast(...args),
  subscribeScopedSyncWakes: (fn) => { fake.woken = fn; return () => {}; },
  subscribeScopedSyncScopes: (fn) => { fake.scopeChanged = fn; return () => {}; },
  projectSyncScope: (projectId, filters = {}) => ({ kind: "project_tasks", project_id: projectId, filters }),
}));
vi.mock("../src/core/scopedSyncEngine.js", () => ({ createScopedSyncEngine: (options) => {
  const engine = { options, track: vi.fn(), untrack: vi.fn(), dispose: vi.fn(), catchUp: vi.fn(async () => true), wake: vi.fn(async () => true) };
  fake.engines.push(engine); return engine;
} }));
vi.mock("../src/core/scopedTaskSyncCache.js", () => ({ createScopedTaskSyncCache: vi.fn(() => ({})) }));
vi.mock("../src/core/localCache.js", () => ({ subscribeCache: (_address, fn) => { fake.cacheChanged = fn; return () => {}; } }));
vi.mock("../src/core/scopedProjectColumns.js", () => ({ refreshScopedProjectColumns: (...args) => fake.columns(...args) }));
vi.mock("../src/core/scopedTaskBody.js", () => ({ refreshScopedTaskBody: (...args) => fake.body(...args) }));
let runtime;
const scope = { kind: "task", task_id: "t1" };
beforeEach(async () => {
  vi.stubGlobal("navigator", { locks: { request: vi.fn() } });
  vi.resetModules(); fake.woken = null; fake.broadcast.mockClear(); fake.columns.mockClear(); fake.body.mockClear(); fake.lock = false; fake.engines = [];
  fake.scopes = [{ scope, projectId: "p1", key: "t1" }];
  fake.contexts = [{ deviceId: "d1", session: {}, cacheScope: {}, active: () => true,
    greeting: { epoch: "e1", current: () => true, capabilities: { sync: { task: true, projectTasks: true } } } }];
  runtime = await import("../src/core/scopedSyncRuntime.js");
});
describe("scoped sync runtime ownership", () => {
  it("does not create an engine for the legacy lock timeout fallback", () => {
    runtime.startScopedSyncRuntime({ ownsLock: () => fake.lock, request: vi.fn() });
    expect(fake.engines).toHaveLength(0);
    expect(runtime.scopedSyncOwns("d1", scope)).toBe(true);
    runtime.stopScopedSyncRuntime();
  });
  it("tracks mounted scopes on grant and fences work immediately on handover", async () => {
    fake.lock = true;
    runtime.startScopedSyncRuntime({ ownsLock: () => fake.lock, request: vi.fn() });
    const engine = fake.engines[0];
    expect(engine.track).toHaveBeenCalledWith(scope);
    expect(engine.options.active()).toBe(true);
    await runtime.captureScopedSync("d1");
    expect(engine.catchUp).toHaveBeenCalledWith(scope);
    fake.lock = false;
    expect(engine.options.active()).toBe(false);
    runtime.reconcileScopedSyncRuntime();
    expect(engine.dispose).toHaveBeenCalledOnce();
    runtime.stopScopedSyncRuntime();
  });
  it("retires old epoch engines and stops checking closed scopes", () => {
    fake.lock = true;
    runtime.startScopedSyncRuntime({ ownsLock: () => fake.lock, request: vi.fn() });
    const engine = fake.engines[0];
    fake.scopes = [];
    fake.scopeChanged();
    expect(engine.untrack).toHaveBeenCalledWith(scope);
    fake.contexts[0].greeting = { ...fake.contexts[0].greeting, epoch: "e2" };
    fake.greeted("d1");
    expect(engine.dispose).toHaveBeenCalledOnce();
    runtime.stopScopedSyncRuntime();
  });
});

it("keeps legacy writers when no browser-wide lock is available", () => {
  vi.stubGlobal("navigator", {});
  expect(runtime.scopedSyncOwns("d1", scope)).toBe(false);
});
it("uses trailing wake scheduling for notifications during an in-flight catch-up", async () => {
  fake.lock = true;
  runtime.startScopedSyncRuntime({ ownsLock: () => fake.lock, request: vi.fn() });
  const engine = fake.engines[0], predicate = () => true;
  await runtime.wakeScopedSync("d1", predicate);
  expect(engine.wake).toHaveBeenCalledWith(predicate);
  runtime.stopScopedSyncRuntime();
});
it("does not infer task capabilities for an unregistered scope kind", () => {
  const unknown = { kind: "future" };
  fake.scopes = [{ scope: unknown, key: "future" }];
  expect(runtime.scopedSyncOwns("d1", unknown)).toBe(false);
});

it("refreshes tracked task body bytes when journal cache writes announce a new head", async () => {
  fake.lock = true;
  runtime.startScopedSyncRuntime({ ownsLock: () => fake.lock, request: vi.fn() });
  fake.body.mockClear();
  fake.cacheChanged?.({ deviceId: "d1", entityId: "p1", kind: "tracker-task", sub: "t1" });
  await vi.waitFor(() => expect(fake.body).toHaveBeenCalledWith(expect.objectContaining({
    deviceId: "d1", projectId: "p1", taskId: "t1", active: expect.any(Function) })));
  runtime.stopScopedSyncRuntime();
});

it("prepares custom columns for a project first opened after the connection pass", async () => {
  fake.lock = true;
  fake.scopes = [];
  runtime.startScopedSyncRuntime({ ownsLock: () => fake.lock, request: vi.fn() });
  const project = { kind: "project_tasks", project_id: "p2", filters: {}, order: "number" };
  fake.scopes = [{ scope: project, projectId: "p2", key: "p2" }];
  fake.scopeChanged();
  await vi.waitFor(() => expect(fake.columns).toHaveBeenCalledWith(expect.objectContaining({ deviceId: "d1", projectId: "p2" })));
  runtime.stopScopedSyncRuntime();
});

it("does not subscribe scoped services on the legacy no-Locks path", () => {
  vi.stubGlobal("navigator", {});
  fake.scopeChanged = null; fake.changed = null; fake.greeted = null; fake.cacheChanged = null;
  runtime.startScopedSyncRuntime({ ownsLock: () => false, request: vi.fn() });
  expect([fake.scopeChanged, fake.changed, fake.greeted, fake.cacheChanged]).toEqual([null, null, null, null]);
  runtime.stopScopedSyncRuntime();
});
it("broadcasts matching tracked scope hints when this tab is a follower", async () => {
  runtime.startScopedSyncRuntime({ ownsLock: () => false, request: vi.fn() });
  await runtime.wakeScopedSync("d1", (scope) => scope.task_id === "t1");
  expect(fake.broadcast).toHaveBeenCalledExactlyOnceWith("d1", [scope]);
  expect(fake.engines).toHaveLength(0);
  runtime.stopScopedSyncRuntime();
});
it("uses received hints only to wake owned tracked scopes without rebroadcasting", async () => {
  fake.lock = true;
  runtime.startScopedSyncRuntime({ ownsLock: () => fake.lock, request: vi.fn() });
  const engine = fake.engines[0];
  await fake.woken?.({ deviceId: "d1", scopes: [scope, { kind: "task", task_id: "other" }] });
  expect(engine.wake).toHaveBeenCalledOnce();
  const predicate = engine.wake.mock.calls[0][0];
  expect(predicate(scope)).toBe(true);
  expect(predicate({ kind: "task", task_id: "other" })).toBe(false);
  expect(fake.broadcast).not.toHaveBeenCalled();
  runtime.stopScopedSyncRuntime();
});
it("forwards only tracked scopes named by a follower's task push", async () => {
  runtime.startScopedSyncRuntime({ ownsLock: () => false, request: vi.fn() });
  await runtime.wakeScopedSyncChanges("d1", [{ entity_id: "p1", tasks: { task_ids: ["t1"] } }]);
  expect(fake.broadcast).toHaveBeenCalledExactlyOnceWith("d1", [scope]);
  fake.broadcast.mockClear();
  await runtime.wakeScopedSyncChanges("d1", [{ entity_id: "other", tasks: { task_ids: ["t1"] } }]);
  expect(fake.broadcast).not.toHaveBeenCalled();
  runtime.stopScopedSyncRuntime();
});
