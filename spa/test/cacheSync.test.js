// @vitest-environment jsdom
// The ordered sync: the one read of the wire this client makes.
//
// A pass is bounded and in a fixed order — the lists, then the workspace the
// reader is standing in, then the rest — and everything past the lists is
// either a small whole shape or a cursored delta. Nothing here is a timer, and
// nothing here reads a conversation or a commit list whole when the cache
// already holds part of it.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const ago = (hours) => new Date(Date.now() - hours * 3600 * 1000).toISOString();

const branchItem = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
  branch: "build/login",
  state: "building",
  anchor: ago(3),
  last_activity: ago(1),
  worktree_id: "wt-1",
  run_id: "run-1",
  issue_id: null,
  agents: [],
  ...over,
});

/** The one bridge this file's device answers through. */
const bridge = { call: null };

/** Where the reader is standing. The routed workspace leads every pass and is
 *  the only one read in front of the foreground. */
const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/app.js", () => ({ App }));

let registeredWatchers = [];
vi.mock("../src/core/changeEvents.js", () => ({
  watchChanges: (registration) => {
    const watcher = { ...registration, disposed: false };
    registeredWatchers.push(watcher);
    return {
      dispose: () => {
        watcher.disposed = true;
      },
    };
  },
}));

const contexts = new Map();
const stateListeners = new Set();
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: (deviceId) => contexts.get(deviceId) || null,
  liveContexts: () => [...contexts.values()],
  onDeviceStateChanged: (fn) => {
    stateListeners.add(fn);
    return () => stateListeners.delete(fn);
  },
}));

const registerDevice = (deviceId, call = (...args) => bridge.call(...args)) => {
  const context = {
    deviceId,
    rpc: call,
    session: { device: deviceId },
    greeted: Promise.resolve(),
    cacheScope: { deviceId, active: () => true },
    active: () => contexts.get(deviceId) === context,
  };
  contexts.set(deviceId, context);
  return context;
};

let cache, sync;

/** What the board answers with, per case. */
let board = [];

/** Whatever a case wants said instead of the shapes below. */
let script = {};

const ANSWERS = {
  "board.list": () => ({ items: board }),
  "project.list": () => ({ projects: [{ project_id: "p1", name: "build" }] }),
  "workspace.list": () => ({ workspaces: [] }),
  "git.status": () => ({ head: "abc", status_key: "key-1", files: [] }),
  "git.log": () => ({ commits: [{ hash: "c1" }], newest: "c1", reset: false }),
  "git.unpushed": () => ({ base: { label: "origin/main" }, commits: [], diff_key: "d1" }),
  "git.show": (params) => ({ hash: params.hash, patch: "diff --git", truncated: false }),
  "term.list": () => ({ terminals: [] }),
  "fs.tree": (params) => ({ path: params.path, entries: [] }),
  "thread.page": () => ({ items: [], has_more: false }),
  "run.diff": () => ({ patch: "the whole diff", diff_key: "d1" }),
  "worktree.diff": () => ({ patch: "the whole diff", diff_key: "d1" }),
};

const answer = (method, params) => (script[method] || ANSWERS[method] || (() => ({})))(params || {});

/** Run every queued turn until the wire goes quiet. A pass is a chain of RPCs
 *  and IndexedDB transactions, each settling on its own macrotask. */
const settle = async () => {
  let before = -1;
  while (before !== bridge.call.mock.calls.length) {
    before = bridge.call.mock.calls.length;
    for (let turn = 0; turn < 12; turn += 1) await new Promise((done) => setTimeout(done, 0));
  }
};

/** One device comes up, is synced, and the wire falls quiet. */
const boot = async (items, route = { name: "inbox" }) => {
  board = items;
  App.route = route;
  sync.startCacheSync();
  await settle();
};

const calls = (method) => bridge.call.mock.calls.filter(([name]) => name === method);
const paramsOf = (method) => calls(method).map(([, params]) => params);
const read = (entityId, kind, sub = "") => cache.readCached({ deviceId: "dev-1", entityId, kind, sub });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete globalThis.navigator?.locks;
  registeredWatchers = [];
  stateListeners.clear();
  contexts.clear();
  board = [];
  script = {};
  App.route = { name: "inbox" };
  registerDevice("dev-1");
  bridge.call = vi.fn(async (method, params) => answer(method, params));
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/cacheSync.js");
});

afterEach(() => {
  sync.stopCacheSync();
  delete globalThis.requestIdleCallback;
});

describe("the order a pass reads in", () => {
  const two = [
    branchItem(),
    branchItem({ branch: "build/search", run_id: "run-2", worktree_id: "wt-2" }),
  ];
  const routeTo = (branch) => ({ name: "branch", deviceId: "dev-1", projectId: "p1", branch });

  it("reads the three lists before it reads any workspace", async () => {
    await boot(two);
    const order = bridge.call.mock.calls.map(([method]) => method);
    const lists = ["board.list", "project.list", "workspace.list"];
    expect(order.slice(0, 3).sort()).toEqual([...lists].sort());
    expect(order.slice(3).some((method) => lists.includes(method))).toBe(false);
  });

  it("reads the routed workspace's shapes before the other's", async () => {
    await boot(two, routeTo("build/search"));
    const entities = bridge.call.mock.calls
      .map(([, params]) => params?.run_id)
      .filter(Boolean);
    expect(entities[0]).toBe("run-2");
    expect(entities.lastIndexOf("run-2")).toBeLessThan(entities.indexOf("run-1"));
  });

  it("reads only the routed workspace ahead of the foreground", async () => {
    await boot(two, routeTo("build/search"));
    const ahead = bridge.call.mock.calls
      .filter(([, , envelope]) => envelope?.priority !== "background")
      .map(([, params]) => params?.run_id || "list");
    expect([...new Set(ahead)]).toEqual(["run-2"]);
  });

  it("puts every read behind the foreground when nothing is routed", async () => {
    await boot(two);
    const ahead = bridge.call.mock.calls.filter(([, , envelope]) => envelope?.priority !== "background");
    expect(ahead).toEqual([]);
  });
});

describe("what a pass writes", () => {
  it("writes the three lists and one row per feed item", async () => {
    script["workspace.list"] = () => ({ workspaces: [{ id: "ws-1", project_id: "p1", name: "wire" }] });
    await boot([branchItem()]);
    expect((await read("", "feed")).value.items).toHaveLength(1);
    expect((await read("", "projects")).value[0].id).toBe("p1");
    expect((await read("", "workspaces")).value[0].id).toBe("ws-1");
    expect((await read("run-1", "row")).value.branch).toBe("build/login");
  });

  it("writes the status, the tree root, the tabs, the log and the unpushed commits", async () => {
    script["term.list"] = () => ({ terminals: [{ term_id: "term-1" }] });
    await boot([branchItem()]);
    expect((await read("run-1", "status")).value.status_key).toBe("key-1");
    expect((await read("run-1", "tree", "")).value.entries).toEqual([]);
    expect((await read("run-1", "terminals")).value.tabs).toEqual([{ term_id: "term-1" }]);
    expect((await read("run-1", "log")).value.newest).toBe("c1");
    expect((await read("run-1", "unpushed")).value.diff_key).toBe("d1");
  });

  it("keeps the unpushed commits' patch out of the record it holds them in", async () => {
    script["git.unpushed"] = () => ({ base: {}, commits: [], diff_key: "d1", patch: "megabytes" });
    await boot([branchItem()]);
    expect((await read("run-1", "unpushed")).value.patch).toBeUndefined();
  });

  it("asks nothing of git for an issue, and still lists its shells and its conversations", async () => {
    await boot([{
      kind: "issue",
      project_id: "p2",
      issue_id: "iss-1",
      state: "plan_review",
      anchor: ago(2),
      last_activity: ago(2),
      agents: [{ id: "ag-1" }],
    }]);
    expect(calls("git.status")).toEqual([]);
    expect(paramsOf("term.list")).toEqual([{ project_id: "p2" }]);
    expect(paramsOf("thread.page")[0]).toMatchObject({ entity_id: "iss-1", agent_id: "ag-1" });
  });
});

describe("the cursors", () => {
  it("reads the log forward from the newest hash it holds", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, { commits: [{ hash: "c0" }], newest: "c0" });
    script["git.log"] = () => ({ commits: [{ hash: "c1" }], newest: "c1", reset: false });
    await boot([branchItem()]);
    expect(paramsOf("git.log")).toEqual([{ run_id: "run-1", since: "c0" }]);
    expect((await read("run-1", "log")).value.commits.map((one) => one.hash)).toEqual(["c1", "c0"]);
  });

  it("asks for the latest twenty when it holds no cursor", async () => {
    await boot([branchItem()]);
    expect(paramsOf("git.log")).toEqual([{ run_id: "run-1", limit: sync.LATEST_COMMITS }]);
    expect(sync.LATEST_COMMITS).toBe(20);
  });

  it("replaces the log outright when the answer says the history moved", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, { commits: [{ hash: "c0" }], newest: "c0" });
    script["git.log"] = () => ({ commits: [{ hash: "r1" }], newest: "r1", reset: true });
    await boot([branchItem()]);
    expect((await read("run-1", "log")).value.commits.map((one) => one.hash)).toEqual(["r1"]);
    // `reset` is what one answer said, not something the record goes on being.
    expect((await read("run-1", "log")).value.reset).toBeUndefined();
  });

  it("reads a conversation forward from the sequence it holds", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 7 } }], deliveredSequence: 7, olderItemsRemain: true },
    );
    script["thread.page"] = () => ({ items: [{ id: "m-2", data: { sequence: 8 } }], has_more: false });
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    expect(paramsOf("thread.page")).toEqual([
      { entity_id: "run-1", agent_id: "ag-1", after_sequence: 7, limit: sync.LATEST_THREAD_ITEMS },
    ]);
    const record = await read("run-1", "thread", "ag-1");
    expect(record.value.items.map((one) => one.id)).toEqual(["m-1", "m-2"]);
    expect(record.value.deliveredSequence).toBe(8);
    // How far back the window reaches is not what a forward page answers.
    expect(record.value.olderItemsRemain).toBe(true);
  });

  it("asks for the latest hundred when it holds no sequence, and remembers what is behind them", async () => {
    script["thread.page"] = () => ({ items: [{ id: "m-1", data: { sequence: 40 } }], has_more: true, thread_total: 900 });
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    expect(paramsOf("thread.page")).toEqual([
      { entity_id: "run-1", agent_id: "ag-1", limit: sync.LATEST_THREAD_ITEMS },
    ]);
    expect(sync.LATEST_THREAD_ITEMS).toBe(100);
    const record = await read("run-1", "thread", "ag-1");
    expect(record.value.olderItemsRemain).toBe(true);
    expect(record.value.deliveredSequence).toBe(40);
  });

  it("stores a conversation under its conversation id where it has one", async () => {
    script["thread.page"] = () => ({ items: [{ id: "m-1", data: { sequence: 1 } }], has_more: false });
    await boot([branchItem({ agents: [{ id: "ag-1", conversation_id: "conv-9" }] })]);
    expect(await read("run-1", "thread", "conv-9")).toBeTruthy();
    expect(await read("run-1", "thread", "ag-1")).toBeUndefined();
  });

  it("never asks for a whole conversation or a whole log on a warm cache", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, { commits: [{ hash: "c0" }], newest: "c0" });
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 7 } }], deliveredSequence: 7 },
    );
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    expect(paramsOf("git.log").every((params) => params.since)).toBe(true);
    expect(paramsOf("thread.page").every((params) => params.after_sequence)).toBe(true);
  });
});

describe("the two bodies that never ride a push", () => {
  const unpushed = (hashes) => ({ base: {}, commits: hashes.map((hash) => ({ hash })), diff_key: "d1" });

  it("reads the patch behind each unpushed commit it does not hold, under the cap", async () => {
    script["git.unpushed"] = () => unpushed(["h1", "h2"]);
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "h1" }, { hash: "h1" });
    await boot([branchItem()]);
    expect(paramsOf("git.show")).toEqual([
      { run_id: "run-1", hash: "h2", max_bytes: sync.COMMIT_PATCH_MAX_BYTES },
    ]);
    expect(sync.COMMIT_PATCH_MAX_BYTES).toBe(262144);
    expect((await read("run-1", "patch", "h2")).value.patch).toBe("diff --git");
  });

  it("reads the patches of at most twenty commits", async () => {
    script["git.unpushed"] = () => unpushed(Array.from({ length: 30 }, (_unused, index) => `h${index}`));
    await boot([branchItem()]);
    expect(calls("git.show")).toHaveLength(sync.UNPUSHED_COMMITS_MAX);
  });

  it("refuses a patch over the cap, whatever the bridge answered", async () => {
    script["git.unpushed"] = () => unpushed(["h1"]);
    script["git.show"] = () => ({ hash: "h1", patch: "x".repeat(sync.COMMIT_PATCH_MAX_BYTES + 1) });
    await boot([branchItem()]);
    expect(await read("run-1", "patch", "h1")).toBeUndefined();
  });

  it("lets go of the patch of a commit that is no longer unpushed", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "gone" }, { hash: "gone" });
    script["git.unpushed"] = () => unpushed([]);
    await boot([branchItem()]);
    expect(await read("run-1", "patch", "gone")).toBeUndefined();
  });

  it("reads the working-tree diff only where the cache holds none", async () => {
    await boot([branchItem()]);
    expect(paramsOf("run.diff")).toEqual([{ run_id: "run-1" }]);
    expect((await read("run-1", "diff")).value.patch).toBe("the whole diff");

    bridge.call.mockClear();
    sync.stopCacheSync();
    sync.startCacheSync();
    await settle();
    expect(calls("run.diff")).toEqual([]);
  });
});

describe("the lifetime rules a pass applies", () => {
  it("ages a Recent workspace's data out, and reads what survived", async () => {
    const old = Date.now() - 100 * 3600 * 1000;
    const clock = vi.spyOn(Date, "now").mockReturnValue(old);
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-2", kind: "status" }, { head: "stale" });
    clock.mockRestore();
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-3", kind: "status" }, { head: "warm" });

    const quiet = { anchor: ago(40), last_activity: ago(30) };
    await boot([
      branchItem(),
      branchItem({ branch: "b2", run_id: "run-2", worktree_id: "wt-2", ...quiet }),
      branchItem({ branch: "b3", run_id: "run-3", worktree_id: "wt-3", ...quiet }),
    ]);

    // run-2's data aged out, so it is cold and nothing was read for it.
    expect(await read("run-2", "status")).toBeUndefined();
    expect(paramsOf("git.status").map((params) => params.run_id)).toEqual(["run-1", "run-3"]);
  });

  // A push says a row finished, and the workspace's data goes with it. A tab
  // that was not open to hear that push boots to a board that already says so,
  // and the rule has to hold there too.
  it("lets go of the data of a row the board lists as over", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-2", kind: "status" }, { head: "old" });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-2", kind: "thread", sub: "ag-1" }, { items: [] });
    await boot([
      branchItem(),
      branchItem({ branch: "b2", run_id: "run-2", worktree_id: "wt-2", state: "merged" }),
    ]);
    expect(await read("run-2", "status")).toBeUndefined();
    expect(await read("run-2", "thread", "ag-1")).toBeUndefined();
    // The row stays: the board is what lists it, and it still does.
    expect(await read("run-2", "row")).toBeTruthy();
  });

  it("takes everything from an entity the board has stopped naming", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-9", kind: "status" }, {});
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-9", kind: "row" }, {});
    await boot([branchItem()]);
    expect(await read("run-9", "status")).toBeUndefined();
    expect(await read("run-9", "row")).toBeUndefined();
    expect(await read("run-1", "row")).toBeTruthy();
  });
});

describe("no timers", () => {
  it("issues nothing in an hour of wall clock", async () => {
    await boot([branchItem()]);
    bridge.call.mockClear();
    vi.useFakeTimers();
    try {
      vi.advanceTimersByTime(60 * 60 * 1000);
    } finally {
      vi.useRealTimers();
    }
    await settle();
    expect(bridge.call).not.toHaveBeenCalled();
  });

  it("registers no watcher that polls", async () => {
    await boot([branchItem()]);
    expect(registeredWatchers.map((watcher) => watcher.intervalMs)).toEqual([undefined, undefined]);
  });
});

describe("when a pass runs", () => {
  it("runs once for a session, however often the device's state is announced", async () => {
    await boot([branchItem()]);
    const first = calls("board.list").length;
    stateListeners.forEach((fn) => fn());
    stateListeners.forEach((fn) => fn());
    await settle();
    expect(calls("board.list")).toHaveLength(first);
  });

  it("runs again for the session a reconnect brought", async () => {
    await boot([branchItem()]);
    contexts.get("dev-1").session = { device: "dev-1", again: true };
    stateListeners.forEach((fn) => fn());
    await settle();
    expect(calls("board.list")).toHaveLength(2);
  });

  it("takes the session up again when the lists did not answer", async () => {
    script["board.list"] = () => {
      throw new Error("the bridge was busy");
    };
    await boot([branchItem()]);
    expect(registeredWatchers).toEqual([]);

    script = {};
    stateListeners.forEach((fn) => fn());
    await settle();
    expect(calls("board.list")).toHaveLength(2);
    expect(registeredWatchers.map((watcher) => watcher.id)).toEqual(["s-inbox", "s-background"]);
  });

  it("runs again when the tab comes back", async () => {
    await boot([branchItem()]);
    document.dispatchEvent(new Event("visibilitychange"));
    await settle();
    expect(calls("board.list")).toHaveLength(2);
  });

  it("leaves the whole job to the tab that holds the lock", async () => {
    vi.stubGlobal("navigator", { locks: { request: vi.fn(async () => undefined) } }); // never granted
    vi.resetModules();
    cache = await import("../src/core/localCache.js");
    sync = await import("../src/core/cacheSync.js");
    await boot([branchItem()]);
    expect(bridge.call).not.toHaveBeenCalled();
    expect(await read("", "feed")).toBeUndefined();
    vi.unstubAllGlobals();
  });
});

describe("every device answers for itself", () => {
  it("reads each device through its own caller and writes under its own id", async () => {
    const second = vi.fn(async (method, params) =>
      method === "git.log" ? { commits: [{ hash: "other" }], newest: "other" } : answer(method, params));
    registerDevice("dev-2", second);
    await boot([branchItem()]);
    expect((await read("run-1", "log")).value.commits[0].hash).toBe("c1");
    expect(
      (await cache.readCached({ deviceId: "dev-2", entityId: "run-1", kind: "log" })).value.commits[0].hash,
    ).toBe("other");
  });

  it("evicts within one device only what that device's board stopped naming", async () => {
    registerDevice("dev-2", vi.fn(async (method, params) => answer(method, params)));
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-9", kind: "status" }, {});
    await cache.writeCached({ deviceId: "dev-2", entityId: "run-9", kind: "status" }, {});
    board = [branchItem({ branch: "b9", run_id: "run-9", worktree_id: "wt-9" })];
    // dev-1 is asked for a board that no longer names run-9; dev-2's does.
    bridge.call = vi.fn(async (method, params) =>
      method === "board.list" ? { items: [branchItem()] } : answer(method, params));
    sync.startCacheSync();
    await settle();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-9", kind: "status" })).toBeUndefined();
    expect(await cache.readCached({ deviceId: "dev-2", entityId: "run-9", kind: "status" })).toBeTruthy();
  });
});
