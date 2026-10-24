// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { called, passesSettled } from "./waits.js";

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
let watchers = [];
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, projectSources: true,
    kinds: ["state", "thread", "git", "files", "terminals", "tasks"] }, term: { projectSources: true } }),
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: (registration) => { watchers.push(registration); return { dispose() {} }; },
}));
let context;
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context, liveContexts: () => [context],
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
  onDeviceStateChanged: () => () => {},
}));
const scope = { project_id: "p1", source_id: "source", source_path: "/repo" };
const projects = ["p1", "p2"].map((id) => ({ project_id: id, sources: [{ id: "source", path: "/repo" }] }));
const board = Array.from({ length: 4 }, (_, index) => ({ kind: "branch", state: "building", project_id: index < 2 ? "p1" : "p2",
  run_id: `run-${index}`, worktree_id: `wt-${index}`, agents: [{ id: `agent-${index}`, conversation_id: `c${index}`, thread_id: `thread:c${index}` }] }));
const address = (entityId, kind, sub = "") => ({ deviceId: "dev-1", entityId, kind, sub });
let cache, sync, active;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  watchers = []; active = true;
  const answers = {
    "board.list": () => ({ items: board }), "project.list": () => ({ projects }), "workspace.list": () => ({ workspaces: [] }),
    "thread.page": (params) => ({ thread_id: params.thread_id, items: [], has_more: false }),
    "term.list": () => ({ terminals: [] }), "tasks.list": () => ({ tasks: [] }), "tasks.columns": () => ({ columns: [] }),
    "fs.tree": () => ({ entries: [] }), "git.status": () => ({ files: [], status_key: "initial" }),
    "git.log": () => ({ commits: [] }), "git.unpushed": () => ({ commits: [] }),
  };
  context = { deviceId: "dev-1", session: {}, greeted: Promise.resolve(), active: () => active,
    cacheScope: { deviceId: "dev-1", active: () => active },
    rpc: vi.fn(async (method, params = {}) => (answers[method] || (() => ({})))(params)) };
  cache = await import("../src/core/localCache.js"); sync = await import("../src/core/cacheSync.js");
  sync.startCacheSync(); await passesSettled(sync, "dev-1");
});
afterEach(async () => { sync.stopCacheSync(); vi.restoreAllMocks(); await passesSettled(sync, "dev-1"); });

const gate = () => {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release };
};
const push = (items) => watchers.find((entry) => entry.kinds.includes("thread")).onChanges(items);
const finishEntity = () => push([{ entity_id: "run-0", state: { ...board[0], state: "merged" } }]);
const heldTip = () => ({ entity_id: "run-0", thread: [{ agent_id: "agent-0", conversation_id: "c0",
  thread_id: "thread:c0", since_sequence: 0, last_sequence: 10, items: [] }] });

it.each(["files", "thread"])("does not resurrect %s when a deferred read answers after finished-state eviction", async (kind) => {
  const held = gate(), previous = context.rpc.getMockImplementation();
  const method = kind === "files" ? "fs.tree" : "thread.page";
  context.rpc.mockClear();
  context.rpc.mockImplementation((name, params) => name === method ? held.promise : previous(name, params));
  const deferred = push(kind === "files" ? [{ entity_id: "run-0", files: { paths: ["new.txt"] } }] : [heldTip()]);
  await called(context.rpc, () => context.rpc.mock.calls.some(([name]) => name === method));
  try { await finishEntity(); }
  finally {
    held.release({ thread_id: "thread:c0", has_more: false, items: [{ type: "message", data: { id: "old", sequence: 10 } }],
      path: "", entries: [{ name: "resurrected" }] });
    await deferred;
  }
  expect(await cache.readCached(address("run-0", kind === "files" ? "tree" : "thread", kind === "files" ? "" : "c0"))).toBeUndefined();
  expect((await cache.readCached(address("run-0", "row"))).value.state).toBe("merged");
});

it("drops a queued old-generation catch-up after reset without reading the new generation", async () => {
  const held = gate(), previous = context.rpc.getMockImplementation();
  context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => method === "tasks.list" ? held.promise : previous(method, params));
  const blockers = push(["p1", "p2"].map((entity_id) => ({ entity_id, tasks: { task_ids: ["held"] } })));
  await called(context.rpc, () => context.rpc.mock.calls.filter(([method]) => method === "tasks.list").length === 2);
  const deferred = push([heldTip()]);
  await vi.waitFor(() => expect(sync.pushWorkInFlight("dev-1").backgroundPending).toBeGreaterThan(0));
  try {
    const { reconcileConversationGeneration } = await import("../src/core/conversationReset.js");
    await reconcileConversationGeneration({ deviceId: "dev-1", entityId: "run-0", forceReset: true,
      agent: { id: "agent-0", conversation_id: "c0", thread_id: "replacement", thread_generation_revision: 2 } });
  } finally { held.release({ tasks: [] }); await blockers; await deferred; }
  expect(context.rpc.mock.calls.filter(([method]) => method === "thread.page")).toHaveLength(0);
  expect((await cache.readCached(address("run-0", "thread", "c0"))).value.thread_id).toBe("replacement");
});

it("does not enqueue enrichment for a terminals-only push", async () => {
  const queueModule = await import("../src/core/pushQueue.js");
  const factory = queueModule.createPushQueue, enqueued = vi.fn();
  sync.stopCacheSync();
  const spy = vi.spyOn(queueModule, "createPushQueue").mockImplementation((options) => {
    const queue = factory(options), enqueue = queue.enqueueApplied;
    if (options.concurrency === 2) queue.enqueueApplied = (...args) => { enqueued(...args); return enqueue(...args); };
    return queue;
  });
  try {
    sync.startCacheSync(); await passesSettled(sync, "dev-1");
    await push([{ entity_id: "run-0", terminals: { tabs: [{ term_id: "user" }] } }]);
    expect(enqueued).not.toHaveBeenCalled();
    expect((await cache.readCached(address("run-0", "terminals"))).value.tabs).toEqual([{ term_id: "user" }]);
  } finally { spy.mockRestore(); }
});

it("discards a queued files job when its owner finishes before admission", async () => {
  const held = gate(), previous = context.rpc.getMockImplementation();
  context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => method === "tasks.list" ? held.promise : previous(method, params));
  const blockers = push(["p1", "p2"].map((entity_id) => ({ entity_id, tasks: { task_ids: ["held"] } })));
  await called(context.rpc, () => context.rpc.mock.calls.filter(([method]) => method === "tasks.list").length === 2);
  const deferred = push([{ entity_id: "run-0", files: { root: { path: "", entries: [{ name: "stale" }] }, paths: [] } }]);
  await vi.waitFor(() => expect(sync.pushWorkInFlight("dev-1").backgroundPending).toBeGreaterThan(0));
  try { await finishEntity(); }
  finally { held.release({ tasks: [] }); await blockers; await deferred; }
  expect(await cache.readCached(address("run-0", "tree"))).toBeUndefined();
});

it("rejects a file-body write stalled before commit when eviction ends its lifetime", async () => {
  const target = address("run-0", "file", "held.txt");
  await cache.writeCached(target, { file: { path: "held.txt", size: 3, mime: "text/plain", content_b64: "b2xk" } });
  const lifetime = await import("../src/core/cacheLifetime.js"), queued = gate(), held = gate();
  const store = lifetime.cacheFileBody, previous = context.rpc.getMockImplementation();
  context.rpc.mockImplementation((method, params) => method === "fs.read"
    ? { path: "held.txt", size: 3, mime: "text/plain", content_b64: "bmV3" } : previous(method, params));
  const stalled = vi.spyOn(lifetime, "cacheFileBody").mockImplementation(async (options) => {
    queued.release(); await held.promise; return store(options);
  });
  const deferred = push([{ entity_id: "run-0", files: { paths: ["held.txt"] } }]);
  await queued.promise;
  try { await finishEntity(); }
  finally { held.release(); await deferred; stalled.mockRestore(); }
  expect(await cache.readCached(target)).toBeUndefined();
});
