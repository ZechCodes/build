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
const tip = (since, last) => ({ entity_id: "run-0", thread: [{ agent_id: "agent-0", conversation_id: "c0",
  thread_id: "thread:c0", since_sequence: since, last_sequence: last, items: [] }] });
const page = (last = 10) => ({ thread_id: "thread:c0", thread_last_sequence: last, has_more: last > 100,
  items: Array.from({ length: Math.min(last, 100) }, (_, index) => ({ type: "message", data: { id: `m${Math.max(0, last - 100) + index + 1}`, sequence: Math.max(0, last - 100) + index + 1 } })) });
const calls = (method) => context.rpc.mock.calls.filter(([name]) => method === name);
const blockBackground = async () => {
  const held = gate(), previous = context.rpc.getMockImplementation();
  context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => method === "tasks.list" ? held.promise
    : method === "thread.page" ? page() : previous(method, params));
  const blockers = ["p1", "p2"].map((entity_id) => push([{ entity_id, tasks: { task_ids: ["held"] } }]));
  await called(context.rpc, () => calls("tasks.list").length === 2);
  return { release: () => held.release({ tasks: [] }), done: Promise.all(blockers) };
};

it.each([false, true])("prioritizes visible catch-up with both enrichment/wire slots held after a pass (mount later: %s)", async (later) => {
  expect(sync.passInFlight("dev-1")).toBeFalsy();
  const background = await blockBackground();
  const visible = await import("../src/core/syncVisibleScopes.js");
  const mount = () => visible.registerSyncThread(() => ({ deviceId: "dev-1", entityId: "run-0", agentId: "agent-0", conversationId: "c0" }));
  let unmount = later ? null : mount();
  const pending = push([tip(5, 10)]);
  try {
    if (later) {
      await vi.waitFor(() => expect(sync.pushWorkInFlight("dev-1").running).toBe(0));
      unmount = mount();
    }
    await vi.waitFor(() => expect(calls("thread.page")).toHaveLength(1), { timeout: 2000 });
    expect(calls("thread.page")[0][2]?.priority).not.toBe("background");
    await vi.waitFor(async () => expect((await cache.readCached(address("run-0", "thread", "c0")))?.value.deliveredSequence).toBe(10));
    expect(calls("tasks.list")).toHaveLength(2);
  } finally { unmount?.(); background.release(); await background.done; await pending; }
});

it("coalesces disjoint cap/gap requests while a read stalls and skips covered trailing goals", async () => {
  const held = gate(), previous = context.rpc.getMockImplementation();
  context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => method === "thread.page" ? held.promise : previous(method, params));
  const pending = [push([tip(1, 2)])];
  await called(context.rpc, () => calls("thread.page").length === 1);
  for (let sequence = 2; sequence <= 100; sequence += 1) pending.push(push([tip(sequence * 2 - 1, sequence * 2)]));
  try {
    await vi.waitFor(() => expect(sync.pushWorkInFlight("dev-1").running).toBe(0));
    expect(sync.pushWorkInFlight("dev-1").backgroundPending).toBeLessThanOrEqual(2);
    expect(sync.threadCatchupsInFlight("dev-1")).toEqual({ running: 1, pending: 1 });
  } finally { held.release(page(200)); await Promise.all(pending); }
  expect(calls("thread.page")).toHaveLength(1);
  expect((await cache.readCached(address("run-0", "thread", "c0"))).value.items.map((item) => item.data.sequence))
    .toEqual(Array.from({ length: 100 }, (_, index) => index + 101));
  await push([tip(199, 200)]);
  expect(calls("thread.page")).toHaveLength(1);
});

it("carries a queued legacy goal through the first page introducing a thread identity", async () => {
  await cache.deleteCached([address("run-0", "thread", "c0")]);
  const held = gate(), previous = context.rpc.getMockImplementation();
  context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => method === "thread.page"
    ? (calls("thread.page").length === 1 ? held.promise : page(10)) : previous(method, params));
  const legacyTip = (last) => { const event = tip(0, last); delete event.thread[0].thread_id; return event; };
  const first = push([legacyTip(2)]);
  await called(context.rpc, () => calls("thread.page").length === 1);
  const second = push([legacyTip(10)]);
  await vi.waitFor(() => expect(sync.threadCatchupsInFlight("dev-1").pending).toBe(1));
  held.release(page(2));
  await Promise.all([first, second]);
  expect(calls("thread.page")).toHaveLength(2);
  expect((await cache.readCached(address("run-0", "thread", "c0"))).value.deliveredSequence).toBe(10);
});

it("does not adopt a reset generation for a pending legacy goal from another cache writer", async () => {
  const target = address("run-0", "thread", "c0");
  await cache.deleteCached([target]);
  const held = gate(), previous = context.rpc.getMockImplementation();
  context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => method === "thread.page" ? held.promise : previous(method, params));
  const legacy = (last) => { const event = tip(0, last); delete event.thread[0].thread_id; return event; };
  const first = push([legacy(2)]);
  await called(context.rpc, () => calls("thread.page").length === 1);
  const second = push([legacy(10)]);
  await vi.waitFor(() => expect(sync.threadCatchupsInFlight("dev-1").pending).toBe(1));
  await cache.writeCached(target, { thread_id: "replacement", thread_generation_revision: 2,
    retired_thread_ids: ["thread:c0"], items: [], deliveredSequence: 0 });
  held.release(page(2)); await Promise.all([first, second]);
  expect(calls("thread.page")).toHaveLength(1);
  expect((await cache.readCached(target)).value).toMatchObject({ thread_id: "replacement", deliveredSequence: 0, items: [] });
});

it("bounds repeated failed goals and renews their budget after same-generation eviction", async () => {
  const previous = context.rpc.getMockImplementation();
  context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => {
    if (method === "thread.page") throw new Error("offline");
    return previous(method, params);
  });
  await push([tip(5, 10)]);
  expect(calls("thread.page")).toHaveLength(2);
  await Promise.all(Array.from({ length: 30 }, () => push([tip(5, 10)])));
  expect(calls("thread.page")).toHaveLength(2);
  const { evictWorkspaceData } = await import("../src/core/cacheLifetime.js");
  await evictWorkspaceData("dev-1", "run-0");
  context.rpc.mockImplementation((method, params) => method === "thread.page" ? page(10) : previous(method, params));
  await push([tip(5, 10)]);
  expect(calls("thread.page")).toHaveLength(3);
  expect((await cache.readCached(address("run-0", "thread", "c0"))).value.deliveredSequence).toBe(10);
});

it("appends a legacy carried tip to its known initial generation", async () => {
  const target = address("run-0", "thread", "c0");
  await cache.writeCached(target, { thread_id: "thread:c0", thread_generation_revision: 1,
    retired_thread_ids: [], deliveredSequence: 2, items: page(2).items });
  const event = tip(2, 3); delete event.thread[0].thread_id;
  event.thread[0].items = [{ type: "message", data: { id: "m3", sequence: 3 } }];
  await push([event]);
  expect((await cache.readCached(target)).value).toMatchObject({ deliveredSequence: 3,
    thread_id: "thread:c0", thread_generation_revision: 1, retired_thread_ids: [] });
});

it("uses canonical catch-up rather than assigning an untagged old tip to a reset generation", async () => {
  const target = address("run-0", "thread", "c0");
  await cache.writeCached(target, { thread_id: "replacement", thread_generation_revision: 3,
    retired_thread_ids: ["old"], items: [], deliveredSequence: 0 });
  const previous = context.rpc.getMockImplementation(); context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => method === "thread.page"
    ? { thread_id: "replacement", thread_last_sequence: 0, items: [], has_more: false } : previous(method, params));
  const event = tip(0, 100); delete event.thread[0].thread_id;
  event.thread[0].items = [{ type: "message", data: { id: "old-message", sequence: 1, body: "old generation" } }];
  await push([event]);
  expect((await cache.readCached(target)).value).toMatchObject({ thread_id: "replacement", deliveredSequence: 0, items: [] });
  expect(calls("thread.page")).toHaveLength(2);
  context.rpc.mockImplementation((method, params) => method === "thread.page"
    ? { ...page(5), thread_id: "replacement" } : previous(method, params));
  const current = tip(0, 5); current.thread[0].thread_id = "replacement";
  await push([current]);
  expect(calls("thread.page")).toHaveLength(3);
  expect((await cache.readCached(target)).value.deliveredSequence).toBe(5);
});
