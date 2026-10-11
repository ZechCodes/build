// @vitest-environment jsdom
// The screen gets the wire first; inbox replicas land before checkout warming.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
const lists = ["board.list", "project.list", "workspace.list"];
const gate = () => {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release };
};

let cache, sync, visible, rpc, script, disposers, abandonedPass;
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
const nonLists = () => rpc.mock.calls.filter(([method]) => !lists.includes(method));
const finish = () => passesSettled(sync, "dev-1");
const start = () => sync.startCacheSync();

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  script = {};
  watchers.length = 0;
  disposers = [];
  abandonedPass = false;
  App.route = routeTo(10);
  rpc = vi.fn(async (method, params = {}) => (script[method] || answers[method] || (() => ({})))(params));
  context = {
    deviceId: "dev-1", rpc, session: {}, greeted: Promise.resolve(), active: () => true,
    cacheScope: { deviceId: "dev-1", active: () => true },
  };
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/cacheSync.js");
  visible = await import("../src/core/syncVisibleScopes.js");
});

afterEach(async () => {
  sync.stopCacheSync();
  disposers.forEach((dispose) => dispose());
  vi.unstubAllGlobals();
  if (!abandonedPass) await finish();
});

const mountRoutedPanes = async () => {
  disposers.push(visible.registerSyncDiff(address("run-10", "diff")));
  disposers.push(visible.registerSyncTree(() => address("run-10", "tree"), () => ["", "src"]));
  await cache.writeCached(address("run-10", "diff"), { diff_key: "old-diff", patch: "held patch" });
  await cache.writeCached(address("run-10", "tree", "src"), { path: "src", entries: [] });
  await cache.writeCached(address("run-10", "tree", "hidden"), { path: "hidden", entries: [] });
};

describe("foreground-first passes", () => {
  it("dispatches the new route while an old foreground status is held and keeps its thread ahead of queued work", async () => {
    const oldStatus = gate();
    const newThread = gate();
    script["git.status"] = (params) => params.run_id === "run-10" ? oldStatus.promise : answers["git.status"]();
    script["thread.page"] = (params) => params.agent_id === "ag-9-b" ? newThread.promise : answers["thread.page"]();
    start();
    await called(rpc, () => calls("git.status").some(([, params]) => params.run_id === "run-10"));
    const before = rpc.mock.calls.length;
    App.route = routeTo(9);
    sync.routeChanged();
    try {
      await vi.waitFor(() => expect(calls("thread.page").some(([, params]) => params.agent_id === "ag-9-b")).toBe(true));
      const next = rpc.mock.calls.slice(before).filter(([method]) => !lists.includes(method));
      expect(next[0].slice(0, 2)).toEqual(["thread.page", expect.objectContaining({ entity_id: "run-9", agent_id: "ag-9-b" })]);
      expect(next[0][2]?.priority).not.toBe("background");
      oldStatus.release(answers["git.status"]());
      // Finish the old drain while B's selected thread is still outstanding.
      // Its completion must not release B's foreground barrier.
      await vi.waitFor(async () => expect((await cache.readCached(address("run-10", "status")))?.value).toBeTruthy());
      expect(calls("git.status").filter(([, params]) => params.run_id !== "run-10")).toHaveLength(0);
      expect(calls("fs.tree")).toHaveLength(0);
      expect(calls("thread.page")).toHaveLength(2);
    } finally {
      oldStatus.release(answers["git.status"]());
      newThread.release(answers["thread.page"]());
      await finish();
    }
  });

  it("keeps a newer ordinary cache write when an older foreground diff shape arrives", async () => {
    const held = gate();
    await mountRoutedPanes();
    script["run.diff"] = (params) => params.run_id === "run-10" ? held.promise : answers["run.diff"]();
    start();
    await called(rpc, () => calls("run.diff").some(([, params]) => params.run_id === "run-10"));
    const newer = { diff_key: "newer-surface", patch: "newer surface patch", files: [{ path: "newer.js" }] };
    try {
      await cache.writeCached(address("run-10", "diff"), newer);
    } finally {
      held.release({ diff_key: "older-read", files: [{ path: "older.js" }] });
      await finish();
    }
    expect((await cache.readCached(address("run-10", "diff"))).value).toMatchObject(newer);
  });

  it("promotes a background status lookup when its workspace opens before dispatch", async () => {
    App.route = { name: "inbox", deviceId: "dev-1" };
    const held = gate();
    const lookupStarted = gate();
    const originalRead = cache.readCached;
    let intercepted = false;
    const lookup = vi.spyOn(cache, "readCached").mockImplementation((target) => {
      if (!intercepted && target.entityId === "run-1" && target.kind === "status") {
        intercepted = true;
        lookupStarted.release();
        return held.promise;
      }
      return originalRead(target);
    });
    start();
    await lookupStarted.promise;
    let completed = false;
    try {
      expect(calls("git.status")).toHaveLength(0);
      App.route = routeTo(1);
      sync.routeChanged();
      // #504: returning to a pending scope waits for its status to land before the tree.
      expect(calls("fs.tree")).toHaveLength(0);
    } finally {
      held.release(undefined);
      lookup.mockRestore();
      let timer;
      completed = await Promise.race([
        finish().then(() => true),
        new Promise((resolve) => { timer = setTimeout(() => resolve(false), 3_000); }),
      ]);
      clearTimeout(timer);
      abandonedPass = !completed;
    }
    expect(completed).toBe(true);
    const status = calls("git.status").find(([, params]) => params.run_id === "run-1");
    expect(status[2]?.priority).not.toBe("background");
  });

  it("promotes a queued background read when its route opens during another foreground barrier", async () => {
    App.route = { name: "inbox", deviceId: "dev-1" };
    const heldLookup = gate(), lookupStarted = gate(), heldOtherStatus = gate();
    const originalRead = cache.readCached;
    let intercepted = false;
    const lookup = vi.spyOn(cache, "readCached").mockImplementation((target) => {
      if (!intercepted && target.entityId === "run-1" && target.kind === "status") {
        intercepted = true;
        lookupStarted.release();
        return heldLookup.promise;
      }
      return originalRead(target);
    });
    script["git.status"] = (params) => params.run_id === "run-2" ? heldOtherStatus.promise : answers["git.status"]();
    start();
    await lookupStarted.promise;
    let completed = false;
    try {
      App.route = routeTo(2);
      sync.routeChanged();
      await called(rpc, () => calls("git.status").some(([, params]) => params.run_id === "run-2"));
      heldLookup.release(undefined);
      // The run-1 loader has reached the barrier while run-2 is foreground.
      for (let turn = 0; turn < 500; turn += 1) await Promise.resolve();
      expect(calls("git.status").some(([, params]) => params.run_id === "run-1")).toBe(false);
      App.route = routeTo(1);
      sync.routeChanged();
    } finally {
      heldLookup.release(undefined);
      heldOtherStatus.release(answers["git.status"]());
      lookup.mockRestore();
      let timer;
      completed = await Promise.race([
        finish().then(() => true),
        new Promise((resolve) => { timer = setTimeout(() => resolve(false), 3_000); }),
      ]);
      clearTimeout(timer);
      abandonedPass = !completed;
    }
    expect(completed).toBe(true);
    const status = calls("git.status").find(([, params]) => params.run_id === "run-1");
    expect(status[2]?.priority).not.toBe("background");
  });

  it("keeps a pushed mounted diff when an older foreground diff reply arrives afterwards", async () => {
    const held = gate();
    await mountRoutedPanes();
    script["run.diff"] = (params) => params.run_id === "run-10" ? held.promise : answers["run.diff"]();
    start();
    await called(rpc, () => calls("run.diff").some(([, params]) => params.run_id === "run-10"));
    try {
      await watchers.find((watcher) => watcher.id === "s-background").onChanges([
        { entity_id: "run-10", git: { diff: { diff_key: "pushed-diff", patch: "newest pushed patch" } } },
      ]);
      expect((await cache.readCached(address("run-10", "diff"))).value.diff_key).toBe("pushed-diff");
    } finally {
      held.release({ diff_key: "old-read", patch: "old read patch" });
      await finish();
    }
    expect((await cache.readCached(address("run-10", "diff"))).value).toMatchObject({
      diff_key: "pushed-diff", patch: "newest pushed patch", stale: false,
    });
  });

  it("keeps a null-diff push's stale marker when an older background diff reply arrives", async () => {
    const held = gate();
    script["run.diff"] = (params) => params.run_id === "run-1" ? held.promise : answers["run.diff"]();
    start();
    await called(rpc, () => calls("run.diff").some(([, params]) => params.run_id === "run-1"));
    try {
      // Another cache writer can land a held body after this cold background
      // request started. The newer invalidation must still own its staleness.
      await cache.writeCached(address("run-1", "diff"), { diff_key: "held-body", patch: "held patch" });
      await watchers.find((watcher) => watcher.id === "s-background").onChanges([
        { entity_id: "run-1", git: { diff: null } },
      ]);
      expect((await cache.readCached(address("run-1", "diff"))).value.stale).toBe(true);
    } finally {
      held.release({ diff_key: "old-background-read", patch: "old background patch" });
      await finish();
    }
    expect((await cache.readCached(address("run-1", "diff"))).value).toMatchObject({
      diff_key: "held-body", patch: "held patch", stale: true,
    });
  });

  it("finishes when a background diff's idle turn makes that same Changes pane foreground", async () => {
    App.route = { name: "inbox", deviceId: "dev-1" };
    let switched = false;
    vi.stubGlobal("requestIdleCallback", (callback) => setTimeout(() => {
      if (!switched) {
        switched = true;
        App.route = routeTo(1);
        disposers.push(visible.registerSyncDiff(address("run-1", "diff")));
      }
      callback();
    }, 0));
    start();
    let timer;
    const completed = await Promise.race([
      finish().then(() => true),
      new Promise((resolve) => { timer = setTimeout(() => resolve(false), 10_000); }),
    ]);
    clearTimeout(timer);
    // An unresolved loader cycle has no timer to release; let teardown
    // invalidate its lifetime rather than waiting on that cyclic promise.
    abandonedPass = !completed;
    expect(switched).toBe(true);
    expect(completed).toBe(true);
    expect(calls("run.diff").some(([, params]) => params.run_id === "run-1")).toBe(true);
  });

  it("dispatches a newly opened conversation while an unrelated background Git reply is still held", async () => {
    const held = gate();
    const extra = {
      ...board[9], branch: "build/11", run_id: "run-11", worktree_id: "wt-11",
      agents: [{ id: "ag-11-b" }],
    };
    script["board.list"] = () => ({ items: board, runs: [extra] });
    script["workspace.list"] = () => ({ workspaces: [...workspaces, {
      ...workspaces[9], id: "ws-11", workspace_id: "ws-11", entity_id: "run-11",
      conversations: [{ conversation_id: "ag-11-b" }],
    }] });
    script["git.status"] = (params) => params.run_id === "run-1" ? held.promise : answers["git.status"]();
    start();
    await called(rpc, () => calls("git.status").some(([, params]) => params.run_id === "run-1"));
    try {
      App.route = routeTo(11);
      sync.routeChanged();
      await vi.waitFor(() => expect(calls("thread.page").some(([, params]) => params.agent_id === "ag-11-b")).toBe(true));
      const selected = calls("thread.page").find(([, params]) => params.agent_id === "ag-11-b");
      expect(selected[2]?.priority).not.toBe("background");
      expect(calls("git.status").find(([, params]) => params.run_id === "run-1")[2]?.priority).toBe("background");
    } finally {
      held.release(answers["git.status"]());
      await finish();
    }
  });

  it("reads the mounted chat's remembered conversation first when the route names no agent", async () => {
    delete App.route.agent;
    disposers.push(visible.registerSyncThread(() => ({ deviceId: "dev-1", entityId: "run-10", agentId: "ag-10-b" })));
    start();
    await finish();
    expect(nonLists()[0].slice(0, 2)).toEqual(["thread.page", expect.objectContaining({ entity_id: "run-10", agent_id: "ag-10-b" })]);
  });

  it("reads the selected conversation, open scope, then inbox before warming ten checkouts", async () => {
    await mountRoutedPanes();
    start();
    await finish();
    const order = nonLists();
    expect(order[0].slice(0, 2)).toEqual(["thread.page", expect.objectContaining({ entity_id: "run-10", agent_id: "ag-10-b" })]);
    expect(order[0][2]?.priority).not.toBe("background");

    const selected = order.findIndex(([method, params]) => method === "thread.page" && params.agent_id === "ag-10-b");
    const otherThreads = order.flatMap(([method, params], index) => method === "thread.page" && params.agent_id !== "ag-10-b" ? [index] : []);
    const taskPages = order.flatMap(([method], index) => method === "tasks.list" ? [index] : []);
    const firstTask = order.findIndex(([method, params]) => method === "tasks.list" && params.project_id === "p2" && !params.cursor);
    expect(selected).toBe(0);
    expect(calls("thread.page")).toHaveLength(20);
    expect(calls("tasks.list")).toHaveLength(4);
    expect(firstTask).toBeGreaterThan(selected);
    expect(firstTask).toBeLessThan(Math.min(...otherThreads));
    expect(order[firstTask][2]?.priority).not.toBe("background");

    for (const method of ["git.status", "fs.tree", "term.list", "git.log", "git.unpushed", "run.diff"]) {
      const index = order.findIndex(([name, params]) => name === method && params.run_id === "run-10");
      expect(index, method).toBeGreaterThan(selected);
      expect(index, method).toBeLessThan(Math.min(...otherThreads));
      expect(order[index][2]?.priority, method).not.toBe("background");
    }
    const expanded = order.findIndex(([method, params]) => method === "fs.tree" && params.run_id === "run-10" && params.path === "src");
    expect(expanded).toBeLessThan(Math.min(...otherThreads));
    const inboxEnd = Math.max(...otherThreads, ...taskPages);
    const otherGit = order.findIndex(([method, params]) => method === "git.status" && params.run_id !== "run-10");
    const hiddenTree = order.findIndex(([method, params]) => method === "fs.tree" && params.run_id === "run-10" && params.path === "hidden");
    expect(otherGit).toBeGreaterThan(inboxEnd);
    expect(hiddenTree).toBeGreaterThan(inboxEnd);
    expect(order[hiddenTree][2]?.priority).toBe("background");
    expect(order[otherGit][2]?.priority).toBe("background");
    for (const index of otherThreads) expect(order[index][2]?.priority).toBe("background");
    expect((await cache.readCached(address("run-10", "diff"))).value.diff_key).toBe("new-diff");
  });

  it("waits for inbox thread answers before issuing another checkout's Git", async () => {
    const held = gate();
    script["thread.page"] = (params) => params.agent_id === "ag-1-a" ? held.promise : answers["thread.page"]();
    start();
    await called(rpc, () => calls("thread.page").some(([, params]) => params.agent_id === "ag-1-a"));
    try {
      expect(calls("git.status").filter(([, params]) => params.run_id !== "run-10")).toHaveLength(0);
    } finally {
      held.release(answers["thread.page"]());
      await finish();
    }
  });

  it("requests absent columns alongside the first open-project task page", async () => {
    const held = gate();
    script["tasks.list"] = (params) => params.project_id === "p2" && !params.cursor ? held.promise : answers["tasks.list"](params);
    start();
    await called(rpc, () => calls("tasks.list").some(([, params]) => params.project_id === "p2"));
    try {
      await vi.waitFor(() => expect(calls("tasks.columns").some(([, params]) => params.project_id === "p2")).toBe(true));
      expect(calls("thread.page")).toHaveLength(1);
      expect(calls("tasks.columns").find(([, params]) => params.project_id === "p2")[2]?.priority).not.toBe("background");
    } finally {
      held.release(answers["tasks.list"]({ project_id: "p2" }));
      await finish();
    }
  });

  it("moves a new route's selected thread ahead of the remaining queue mid-pass", async () => {
    const held = gate();
    script["git.status"] = (params) => params.run_id === "run-10" ? held.promise : answers["git.status"]();
    start();
    await called(rpc, () => calls("git.status").some(([, params]) => params.run_id === "run-10"));
    const before = rpc.mock.calls.length;
    App.route = routeTo(9);
    held.release(answers["git.status"]());
    await finish();
    const next = rpc.mock.calls.slice(before).filter(([method]) => !lists.includes(method))[0];
    expect(next.slice(0, 2)).toEqual(["thread.page", expect.objectContaining({ entity_id: "run-9", agent_id: "ag-9-b" })]);
    expect(next[2]?.priority).not.toBe("background");
  });
});

describe("bounded background streams", () => {
  it("keeps at most two background RPCs in flight across ten workspaces and preserves each checkout's read order", async () => {
    const held = gate();
    script["git.status"] = (params) => params.run_id === "run-10" ? answers["git.status"]() : held.promise;
    const original = rpc.getMockImplementation();
    let inFlight = 0, peak = 0;
    rpc.mockImplementation(async (method, params, envelope) => {
      const background = envelope?.priority === "background";
      if (background) peak = Math.max(peak, ++inFlight);
      try { return await original(method, params, envelope); }
      finally { if (background) inFlight -= 1; }
    });
    start();
    try {
      await vi.waitFor(() => expect(calls("git.status").filter(([, params]) => params.run_id !== "run-10")).toHaveLength(2));
      expect(inFlight).toBe(2);
      expect(peak).toBe(2);
    } finally {
      held.release(answers["git.status"]());
      await finish();
    }
    expect(peak).toBe(2);
    expect(inFlight).toBe(0);
    for (let number = 1; number < 10; number += 1) {
      expect(nonLists().filter(([, params]) => params.run_id === `run-${number}`).map(([method]) => method)).toEqual([
        "git.status", "fs.tree", "term.list", "git.log", "git.unpushed", "run.diff",
      ]);
    }
  });

  it("advances other workspaces through a free stream while one status remains slow", async () => {
    const held = gate();
    script["git.status"] = (params) => params.run_id === "run-1" ? held.promise : answers["git.status"]();
    start();
    await called(rpc, () => calls("git.status").some(([, params]) => params.run_id === "run-1"));
    try {
      await vi.waitFor(async () => expect((await cache.readCached(address("run-3", "unpushed")))?.value).toBeTruthy());
      expect(calls("fs.tree").some(([, params]) => params.run_id === "run-1")).toBe(false);
    } finally {
      held.release(answers["git.status"]());
      await finish();
    }
  });

  it("dispatches foreground while both streams are occupied and holds their next reads behind it", async () => {
    const heldOne = gate(), heldTwo = gate(), heldForeground = gate();
    script["git.status"] = (params) => ({ "run-1": heldOne, "run-2": heldTwo, "run-9": heldForeground }[params.run_id]?.promise
      || answers["git.status"]());
    start();
    await called(rpc, () => calls("git.status").some(([, params]) => params.run_id === "run-1"));
    try {
      await vi.waitFor(() => expect(calls("git.status").some(([, params]) => params.run_id === "run-2")).toBe(true));
      const before = rpc.mock.calls.length;
      App.route = routeTo(9);
      sync.routeChanged();
      await called(rpc, () => calls("git.status").some(([, params]) => params.run_id === "run-9"));
      const selected = rpc.mock.calls[before];
      expect(selected.slice(0, 2)).toEqual(["git.status", expect.objectContaining({ run_id: "run-9" })]);
      expect(selected[2]?.priority).not.toBe("background");
      heldTwo.release(answers["git.status"]());
      await vi.waitFor(async () => expect((await cache.readCached(address("run-2", "status")))?.value).toBeTruthy());
      expect(calls("fs.tree").some(([, params]) => params.run_id === "run-2")).toBe(false);
    } finally {
      heldOne.release(answers["git.status"]());
      heldTwo.release(answers["git.status"]());
      heldForeground.release(answers["git.status"]());
      await finish();
    }
  });

  it("cancels both streams and the queued follow-up on stop without writing their late answers", async () => {
    const held = gate();
    script["git.status"] = (params) => params.run_id === "run-10" ? answers["git.status"]() : held.promise;
    start();
    const pass = sync.passInFlight("dev-1");
    try {
      await vi.waitFor(() => expect(calls("git.status").filter(([, params]) => params.run_id !== "run-10")).toHaveLength(2));
      const followUp = sync.syncDevice("dev-1");
      const before = rpc.mock.calls.length;
      sync.stopCacheSync();
      held.release(answers["git.status"]());
      expect(await Promise.all([pass, followUp])).toEqual([false, false]);
      expect(rpc.mock.calls).toHaveLength(before);
      expect(await cache.readCached(address("run-1", "status"))).toBeUndefined();
      expect(await cache.readCached(address("run-2", "status"))).toBeUndefined();
    } finally {
      held.release(answers["git.status"]());
      await pass;
    }
  });
});

describe("list replies paint independently", () => {
  it.each([
    ["board.list", "feed", "items", 10],
    ["project.list", "projects", null, 2],
    ["workspace.list", "workspaces", null, 10],
  ])("writes %s while both other list replies are still out", async (method, kind, collection, count) => {
    const held = gate();
    for (const name of lists.filter((name) => name !== method)) {
      script[name] = async () => { await held.promise; return answers[name](); };
    }
    start();
    try {
      await vi.waitFor(async () => {
        const record = await cache.readCached(address("", kind));
        expect(record).toBeTruthy();
        expect(collection ? record.value[collection] : record.value).toHaveLength(count);
      });
      expect(nonLists()).toHaveLength(0);
    } finally {
      held.release();
      await finish();
    }
  });
});
