// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { passesSettled } from "./waits.js";

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
const page = (through, items) => ({ thread_id: "thread:c0", thread_last_sequence: through, items, has_more: false });

it("does not multiply an unavailable historical read within one ordinary recovery pass", async () => {
  await cache.writeCached(target, { thread_id: "thread:c0", items: [message(1)], deliveredSequence: 3, repairThrough: 3 });
  const previous = context.rpc.getMockImplementation();
  context.rpc.mockClear();
  context.rpc.mockImplementation((method, params) => {
    if (method !== "thread.page" || params.entity_id !== "run-0") return previous(method, params);
    if (params.newest) return page(3, []);
    throw new Error("historical service temporarily unavailable");
  });
  await sync.syncDevice("dev-1");
  const historical = context.rpc.mock.calls.filter(([method, params]) =>
    method === "thread.page" && params.entity_id === "run-0" && !params.newest);
  expect(historical).toHaveLength(1);
  expect((await cache.readCached(target)).value).toMatchObject({ deliveredSequence: 3, repairThrough: 3 });
});

it("commits consecutive carried tips while an ordinary historical repair remains held", async () => {
  await cache.writeCached(target, { thread_id: "thread:c0", items: [message(1), message(3)], deliveredSequence: 3, repairThrough: 3 });
  let entered, release, released = false;
  const began = new Promise((resolve) => { entered = resolve; });
  const waiting = new Promise((resolve) => { release = () => {
    released = true;
    resolve(page(3, [message(1, 3), message(3)]));
  }; });
  releases.push(release);
  const previous = context.rpc.getMockImplementation();
  context.rpc.mockImplementation((method, params) => {
    if (method !== "thread.page" || params.entity_id !== "run-0") return previous(method, params);
    if (params.newest) return page(3, []);
    entered();
    return waiting;
  });
  work.push(sync.syncDevice("dev-1"));
  await began;
  for (const sequence of [4, 5]) {
    const pending = watchers.find((entry) => entry.kinds.includes("thread")).onChanges([{
      entity_id: "run-0", thread: [{ agent_id: "agent-0", conversation_id: "c0", thread_id: "thread:c0",
        since_sequence: sequence - 1, last_sequence: sequence, items: [message(sequence)] }],
    }]);
    work.push(pending);
    await pending;
    expect((await cache.readCached(target)).value).toMatchObject({ deliveredSequence: sequence, repairThrough: 3 });
    expect(released).toBe(false);
  }
  expect(sync.pushWorkInFlight("dev-1")).toMatchObject({ running: 0, pending: 0 });
  release();
  await Promise.all(work);
  expect((await cache.readCached(target)).value).toMatchObject({ deliveredSequence: 5 });
  expect((await cache.readCached(target)).value.repairThrough).toBeUndefined();
});
