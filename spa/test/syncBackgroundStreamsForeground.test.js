// @vitest-environment jsdom
// Real ten-workspace planner and serialized pushes sharing the wire admissions.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { called, passesSettled } from "./waits.js";

const App = { route: {}, devices: [{ id: "dev-1" }] };
const watchers = [];
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({
    changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] },
    tasks: { listPaged: true },
    threads: { leanActivity: false },
  }),
  subscriptionsSettledFor: async () => {},
  onSubscriptionHeld: () => () => {},
  watchChanges: (options) => { watchers.push(options); return { dispose() {} }; },
}));

let context;
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context,
  liveContexts: () => [context],
  onDeviceStateChanged: () => () => {},
}));

const projects = [{ project_id: "p1", name: "One" }, { project_id: "p2", name: "Two" }];
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

let cache, sync, rpc, script, wireStreams;
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
  watchers.length = 0;
  App.route = routeTo(10);
  rpc = vi.fn(async (method, params = {}) => (script[method] || answers[method] || (() => ({})))(params));
  context = {
    deviceId: "dev-1", rpc, session: {}, greeted: Promise.resolve(), active: () => true,
    cacheScope: { deviceId: "dev-1", active: () => true },
  };
  cache = await import("../src/core/localCache.js");
  const scheduler = await import("../src/core/syncBackgroundStreams.js");
  const createStreams = scheduler.createBackgroundStreams;
  const factorySpy = vi.spyOn(scheduler, "createBackgroundStreams").mockImplementation(() => {
    const factory = createStreams();
    const forContext = factory.forContext;
    factory.forContext = (owner) => (wireStreams = forContext(owner));
    return factory;
  });
  sync = await import("../src/core/cacheSync.js");
  factorySpy.mockRestore();
});

afterEach(async () => {
  sync.stopCacheSync();
  vi.unstubAllGlobals();
  await finish();
});

it("allows a serialized push continuation to run while unrelated pass admissions await foreground", async () => {
  App.route = { name: "inbox", deviceId: "dev-1" };
  await cache.writeCached(address("p2", "tracker-tasks"), {
    tasks: [], columns: [{ id: "backlog", name: "Backlog" }],
  });
  const inboxThread = gate(), pushedFirstPage = gate(), pushedTree = gate();
  const pushTreeQueued = gate(), bothPassReadersQueued = gate(), bothPassReadersAtBarrier = gate();
  const foldReached = gate(), allowFold = gate(), selectedTaskRequested = gate(), continuationQueued = gate();
  // Hold the LAST inbox thread so releasing it starts p1's task reads directly.
  script["thread.page"] = (params) => params.agent_id === "ag-10-b"
    ? inboxThread.promise : answers["thread.page"]();
  script["tasks.list"] = (params) => params.project_id === "p2" && !params.cursor
    ? pushedFirstPage.promise : answers["tasks.list"](params);
  script["fs.tree"] = (params) => params.run_id === "run-2"
    ? pushedTree.promise : answers["fs.tree"](params);

  const tracker = await import("../src/core/trackerPages.js");
  const foldTasks = tracker.foldTasksPage;
  let foldIntercepted = false;
  const folding = vi.spyOn(tracker, "foldTasksPage").mockImplementation(async (...args) => {
    if (!foldIntercepted && args[0].entityId === "p2") {
      foldIntercepted = true;
      foldReached.release();
      await allowFold.promise;
    }
    return foldTasks(...args);
  });
  const taskPhases = await import("../src/core/syncTaskPhases.js");
  const createTaskPhases = taskPhases.createTaskPhases;
  const selectedTask = vi.spyOn(taskPhases, "createTaskPhases").mockImplementation((...args) => {
    const phases = createTaskPhases(...args);
    const first = phases.first;
    phases.first = (projectId, ...rest) => {
      if (projectId === "p2") selectedTaskRequested.release();
      return first(projectId, ...rest);
    };
    return phases;
  });

  const traffic = backgroundTraffic();
  let taskPush, treePush, admission, pass;
  let waitingForTree = false, countingPassTasks = false, passAttempts = 0, dispatchChecks = 0, expectingContinuation = false;
  const observePush = (operation, belongsToPass) => {
    if (belongsToPass || operation.priority()?.priority !== "background") return;
    if (waitingForTree) pushTreeQueued.release();
    if (expectingContinuation) continuationQueued.release();
  };
  const observePass = (operation, observation) => {
    if (!countingPassTasks || !observation.belongsToPass) return;
    if (observation.beforeCount === 1 && operation.priority()?.priority === "background") {
      observation.countedPassTask = true;
      if (++passAttempts === 2) bothPassReadersQueued.release();
    }
    if (observation.countedPassTask && observation.beforeCount === 2) {
      if (++dispatchChecks === 2) bothPassReadersAtBarrier.release();
    }
  };
  try {
    start();
    pass = sync.passInFlight("dev-1");
    await called(rpc, () => calls("thread.page").some(([, params]) => params.agent_id === "ag-10-b"));
    const admit = wireStreams.read;
    admission = vi.spyOn(wireStreams, "read").mockImplementation((operation) => {
      const observation = { beforeCount: 0, belongsToPass: false, countedPassTask: false };
      return admit({
        ...operation,
        before: () => {
          observation.beforeCount += 1;
          const waiting = operation.before?.();
          observation.belongsToPass ||= waiting instanceof Promise;
          observePush(operation, observation.belongsToPass);
          observePass(operation, observation);
          return waiting;
        },
      });
    });
    const inboxWatcher = watchers.find((entry) => entry.id === "s-inbox");
    taskPush = inboxWatcher.onChanges([{ entity_id: "p2", tasks: { task_ids: [] } }]);
    await called(rpc, () => calls("tasks.list").some(([, params]) => params.project_id === "p2"));
    waitingForTree = true;
    treePush = watchers.find((entry) => entry.id === "s-background").onChanges([
      { entity_id: "run-2", files: { paths: ["changed.js"] } },
    ]);
    await pushTreeQueued.promise;
    for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
    waitingForTree = false;
    countingPassTasks = true;
    inboxThread.release(answers["thread.page"]());
    await called(rpc, () => calls("fs.tree").some(([, params]) => params.run_id === "run-2"));
    await bothPassReadersQueued.promise;
    for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
    expect(calls("tasks.list").filter(([, params]) => params.project_id === "p1")).toHaveLength(0);
    expect(calls("tasks.columns").filter(([, params]) => params.project_id === "p1")).toHaveLength(0);

    App.route = routeTo(10);
    sync.routeChanged();
    await selectedTaskRequested.promise;
    // Releasing both already-dispatched pushes grants both slots to the
    // queued p1 first-page/columns reads. Their second planner checks now wait
    // for p2 task-first, which joined the actual serialized task push queue.
    pushedTree.release(answers["fs.tree"]({ path: "" }));
    pushedFirstPage.release(answers["tasks.list"]({ project_id: "p2" }));
    await bothPassReadersAtBarrier.promise;
    await foldReached.promise;
    expectingContinuation = true;
    allowFold.release();
    await continuationQueued.promise;
    expect(traffic.pending).toBe(0);
    expect(traffic.peak).toBeLessThanOrEqual(2);
    await vi.waitFor(() => expect(calls("tasks.list").some(([, params]) =>
      params.project_id === "p2" && params.cursor)).toBe(true), { timeout: 1_000, interval: 10 });
  } finally {
    inboxThread.release(answers["thread.page"]());
    pushedFirstPage.release(answers["tasks.list"]({ project_id: "p2" }));
    pushedTree.release(answers["fs.tree"]({ path: "" }));
    allowFold.release();
    App.route = { name: "inbox", deviceId: "dev-1" };
    sync.routeChanged();
    await Promise.all([taskPush, treePush, pass]);
    await finish();
    admission?.mockRestore();
    folding.mockRestore();
    selectedTask.mockRestore();
  }
}, 15_000);
