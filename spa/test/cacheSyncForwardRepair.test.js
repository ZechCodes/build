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
let releases, work, unmount;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  watchers = []; active = true; releases = []; work = []; unmount = null;
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
afterEach(async () => {
  for (const release of releases) release();
  await Promise.allSettled(work);
  unmount?.(); sync.stopCacheSync(); vi.restoreAllMocks();
  await passesSettled(sync, "dev-1");
});

const target = address("run-0", "thread", "c0");
const message = (sequence, updated = sequence) => ({ type: "message",
  data: { sequence, updated_sequence: updated, body: `message ${sequence} at ${updated}` } });
const tip = (since, last) => ({ entity_id: "run-0", thread: [{ agent_id: "agent-0", conversation_id: "c0",
  thread_id: "thread:c0", since_sequence: since, last_sequence: last, items: [] }] });
const push = (item) => {
  const pending = watchers.find((entry) => entry.kinds.includes("thread")).onChanges([item]);
  work.push(pending);
  return pending;
};
const page = (through, items) => ({ thread_id: "thread:c0", thread_last_sequence: through, items, has_more: false });
const current = async () => (await cache.readCached(target))?.value;

async function heldRepair() {
  const visible = await import("../src/core/syncVisibleScopes.js");
  unmount = visible.registerSyncThread(() => ({ deviceId: "dev-1", entityId: "run-0", agentId: "agent-0", conversationId: "c0" }));
  await cache.writeCached(target, { thread_id: "thread:c0", items: [message(1)], deliveredSequence: 1 });
  let release, entered, released = false;
  const waiting = new Promise((resolve) => { release = () => { released = true; resolve(page(5, [message(1, 5), message(3), message(5)])); }; });
  const began = new Promise((resolve) => { entered = resolve; });
  releases.push(release);
  const previous = context.rpc.getMockImplementation();
  context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => {
    if (method !== "thread.page") return previous(method, params);
    if (params.newest) return page(params.after_sequence === 1 ? 3 : 5, [message(params.after_sequence === 1 ? 3 : 5)]);
    entered();
    return waiting;
  });
  push(tip(1, 3));
  await began;
  expect(await current()).toMatchObject({ deliveredSequence: 3, repairThrough: 3 });
  return { release, released: () => released };
}

it("commits a newer visible high-water while an older recent-item repair is held", async () => {
  const repair = await heldRepair();
  push(tip(3, 5));
  await vi.waitFor(async () => expect((await current()).deliveredSequence).toBe(5), { timeout: 10000 });
  expect(repair.released()).toBe(false);
  expect(await current()).toMatchObject({ repairThrough: 5 });
  expect((await current()).items.find((item) => item.data.sequence === 1)).toEqual(message(1));
  expect(context.rpc.mock.calls.filter(([method, params]) => method === "thread.page" && !params.newest)).toHaveLength(1);
  repair.release();
  await Promise.all(work);
  expect((await current()).repairThrough).toBeUndefined();
  expect((await current()).items.find((item) => item.data.sequence === 1)).toEqual(message(1, 5));
});

it("commits a real assignment action while a background recent-item repair is held", async () => {
  const repair = await heldRepair();
  const task = { id: "task-action", project_id: "p1", title: "Immediate assignment", status: "ready",
    assignee: { kind: "agent", agent_id: "agent-0" }, updated_at: "2026-10-25T00:00:00Z" };
  const previous = context.rpc.getMockImplementation();
  context.rpc.mockImplementation((method, params) => method === "tasks.assign" ? { task, timeline: [] } : previous(method, params));
  const { createTaskWaitingOnActions } = await import("../src/core/taskWaitingOnActions.js");
  const actions = createTaskWaitingOnActions({ deviceId: "dev-1", projectId: "p1", taskId: task.id, call: context.rpc });
  await actions.assign({ assignee: task.assignee });
  expect(context.rpc).toHaveBeenCalledWith("tasks.assign", { task_id: task.id, assignee: task.assignee });
  const tracker = await import("../src/core/trackerCache.js");
  expect((await cache.readCached(tracker.taskAddress("dev-1", "p1", task.id)))?.value.task).toEqual(task);
  expect(repair.released()).toBe(false);
});

it.each(["reset", "source", "lifetime"])("refuses an old held repair after its %s fence changes", async (fence) => {
  const repair = await heldRepair();
  if (fence === "reset") {
    const { reconcileConversationGeneration } = await import("../src/core/conversationReset.js");
    await reconcileConversationGeneration({ deviceId: "dev-1", entityId: "run-0", agent: { id: "agent-0", conversation_id: "c0",
      thread_id: "replacement", thread_generation_revision: 2 } });
  } else if (fence === "source") context.cacheScope = {};
  else sync.stopCacheSync();
  const before = await current();
  repair.release();
  await Promise.all(work);
  expect(await current()).toEqual(before);
});
