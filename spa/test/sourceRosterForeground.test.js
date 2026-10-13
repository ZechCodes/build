// @vitest-environment jsdom
// Regression for #535: source terminal replicas cannot hold the foreground wire.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { called, passesSettled } from "./waits.js";

const App = { route: {}, devices: [{ id: "dev-1" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({
    changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] },
    tasks: { listPaged: true },
    term: { projectSources: true },
  }),
  subscriptionsSettledFor: async () => {},
  onSubscriptionHeld: () => () => {},
  watchChanges: () => ({ dispose() {} }),
}));

let context;
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context,
  liveContexts: () => [context],
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
  onDeviceStateChanged: () => () => {},
}));

const projects = [
  { project_id: "p1", name: "One", sources: [
    { id: "first", path: "/unrelated/first" }, { id: "second", path: "/unrelated/second" },
  ] },
  { project_id: "p2", name: "Two", sources: [{ id: "shown-project", path: "/two" }] },
];
const board = Array.from({ length: 10 }, (_, index) => {
  const number = index + 1;
  return {
    kind: "branch", project_id: number > 5 ? "p2" : "p1", branch: `build/${number}`,
    run_id: `run-${number}`, worktree_id: `wt-${number}`, state: "building",
    anchor: new Date().toISOString(), last_activity: new Date().toISOString(),
    agents: ["a", "b"].map((suffix) => ({ id: `ag-${number}-${suffix}` })),
  };
});
const workspaces = board.map((row, index) => ({
  id: `ws-${index + 1}`, workspace_id: `ws-${index + 1}`, name: `Workspace ${index + 1}`,
  project_id: row.project_id, entity_id: row.run_id, status: "ready",
  conversations: row.agents.map((agent) => ({ conversation_id: agent.id })),
}));
const routeTo = (number) => ({
  name: "workspace", deviceId: "dev-1", projectId: number > 5 ? "p2" : "p1",
  workspaceId: `ws-${number}`, agent: `ag-${number}-b`, tab: "changes",
});
const address = (entityId, kind, sub = "") => ({ deviceId: "dev-1", entityId, kind, sub });
const gate = () => {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release };
};

let cache, sync, rpc, script;
const answers = {
  "board.list": () => ({ items: board }),
  "project.list": () => ({ projects }),
  "workspace.list": () => ({ workspaces }),
  "git.status": () => ({ status_key: "status", head: "head", files: [] }),
  "fs.tree": ({ path }) => ({ path, entries: [] }),
  "term.list": () => ({ terminals: [] }),
  "git.log": () => ({ commits: [], newest: "head" }),
  "git.unpushed": () => ({ commits: [], base: {}, diff_key: "diff" }),
  "thread.page": () => ({ items: [], has_more: false }),
  "run.diff": () => ({ diff_key: "new-diff", patch: "new patch" }),
  "tasks.columns": ({ project_id }) => ({ project_id, columns: [{ id: "backlog", name: "Backlog" }] }),
  "tasks.list": ({ project_id, cursor }) => ({
    project_id,
    tasks: [{ id: `${project_id}-${cursor ? "last" : "first"}`, number: cursor ? 1 : 2, status: "backlog" }],
    ...(cursor ? {} : { next_cursor: "last" }),
  }),
};
const calls = (method) => rpc.mock.calls.filter(([name]) => name === method);
const finish = () => passesSettled(sync, "dev-1");
const start = () => sync.startCacheSync();
const backgroundTraffic = () => {
  const original = rpc.getMockImplementation();
  const traffic = { pending: 0, peak: 0 };
  rpc.mockImplementation(async (method, params, envelope) => {
    const background = envelope?.priority === "background";
    if (background) traffic.peak = Math.max(traffic.peak, ++traffic.pending);
    try { return await original(method, params, envelope); }
    finally { if (background) traffic.pending -= 1; }
  });
  return traffic;
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  script = {};
  App.route = routeTo(10);
  rpc = vi.fn(async (method, params = {}) => (script[method] || answers[method] || (() => ({})))(params));
  context = {
    deviceId: "dev-1", rpc, session: {}, greeted: Promise.resolve(), active: () => true,
    cacheScope: { deviceId: "dev-1", active: () => true },
  };
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/cacheSync.js");
});

afterEach(async () => {
  sync.stopCacheSync();
  vi.unstubAllGlobals();
  await finish();
});

const sourceCalls = () => calls("term.list").filter(([, params]) => params.source_id);
const expectWorkspaceCached = async (number) => {
  expect((await cache.readCached(address(`run-${number}`, "thread", `ag-${number}-b`)))?.value).toBeDefined();
  expect((await cache.readCached(address(`run-${number}`, "status")))?.value.status_key).toBe("status");
  expect((await cache.readCached(address(`run-${number}`, "tree")))?.value.entries).toEqual([]);
  expect((await cache.readCached(address(`run-${number}`, "diff")))?.value.diff_key).toBe("new-diff");
};

it.each(["initial", "reconnect"])("refreshes foreground before unrelated source rosters on %s sync", async (mode) => {
  if (mode === "reconnect") {
    start();
    await finish();
    rpc.mockClear();
    context.session = {};
  }
  const roster = gate();
  script["term.list"] = (params) => params.source_id ? roster.promise : answers["term.list"]();
  const traffic = backgroundTraffic();
  if (mode === "initial") start();
  else void sync.syncDevice("dev-1");
  try {
    await called(rpc, () => sourceCalls().length > 0);
    await expectWorkspaceCached(10);
    expect(calls("thread.page").some(([, params]) => params.agent_id === "ag-10-b")).toBe(true);
    expect(calls("tasks.list").some(([, params]) => params.project_id === "p2" && !params.cursor)).toBe(true);
    const foreground = rpc.mock.calls.findIndex(([method, params]) => method === "thread.page" && params.agent_id === "ag-10-b");
    const source = rpc.mock.calls.findIndex(([method, params]) => method === "term.list" && params.source_id);
    expect(foreground).toBeLessThan(source);
    expect(traffic.peak).toBeLessThanOrEqual(2);
  } finally {
    roster.release({ terminals: [] });
    await finish();
  }
});

it("promotes a new workspace while unrelated source responses occupy both background slots", async () => {
  const roster = gate();
  script["term.list"] = (params) => params.project_id === "p1" ? roster.promise : answers["term.list"]();
  const traffic = backgroundTraffic();
  start();
  try {
    await vi.waitFor(() => expect(sourceCalls().filter(([, params]) => params.project_id === "p1")).toHaveLength(2));
    App.route = routeTo(9);
    sync.routeChanged();
    await vi.waitFor(async () => {
      await expectWorkspaceCached(9);
      expect(calls("git.status").find(([, params]) => params.run_id === "run-9")?.[2]?.priority).not.toBe("background");
    });
    expect(traffic.pending).toBe(2);
    expect(traffic.peak).toBeLessThanOrEqual(2);
  } finally {
    roster.release({ terminals: [] });
    await finish();
  }
});
