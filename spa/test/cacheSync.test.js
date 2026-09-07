// The cache's sync layer: one tab holds the lock and follows the feed —
// persisting the snapshot, evicting entities the active set stops naming, and
// keeping every active branch's git status, commit list and file bodies warm.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { FIRST_PAGE_ITEMS } from "../src/core/thread.js";
import { patchFor, worktreeOf } from "./gitWireFixture.js";

// Real clock, not a frozen one: the syncer partitions active-vs-Recent with
// Date.now(), so the items' ages must be relative to the same now.
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
  primary: false,
  ...over,
});

let feedSubscriber = null;
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    feedSubscriber = fn;
    return () => {
      feedSubscriber = null;
    };
  },
}));

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

const App = { session: { deviceId: "dev-1" }, call: vi.fn(async () => ({})) };
vi.mock("../src/app.js", () => ({ App }));

let cache, sync;

const flush = async () => {
  // Generous: a refresh is an RPC pair, then two IndexedDB transactions, each
  // settling on its own macrotask under fake-indexeddb — and the file bodies
  // are warmed a turn behind that.
  for (let i = 0; i < 40; i++) await new Promise((done) => setTimeout(done, 0));
};

const warmTree = worktreeOf({ "src/a.js": "new line" });
const warmStatus = () => warmTree.status({ head: "abc", stat: { insertions: 1, deletions: 0 } });

const snapshot = (items) => ({ items, plans: [], runs: [], externalWorktrees: [], projects: [], primaryChanges: [] });

const feed = async (items) => {
  feedSubscriber(snapshot(items));
  await flush();
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete globalThis.navigator?.locks;
  registeredWatchers = [];
  feedSubscriber = null;
  App.session = { deviceId: "dev-1" };
  App.call = vi.fn(async (method) => {
    if (method === "git.status") return warmStatus();
    if (method === "git.log") return { commits: [{ hash: "abc" }], more: false };
    return {};
  });
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/cacheSync.js");
});

afterEach(() => {
  sync.stopCacheSync();
});

describe("following the feed", () => {
  it("persists each snapshot under the session's device", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "", kind: "feed" });
    expect(record.value.items).toHaveLength(1);
  });

  it("keeps active entities' records and evicts what the active set stops naming", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-2", kind: "status" }, {});
    sync.startCacheSync();
    // run-2 went quiet (Recent) — its cache leaves with it, run-1 stays.
    await feed([
      branchItem(),
      branchItem({ branch: "b2", run_id: "run-2", worktree_id: "wt-2", anchor: ago(40), last_activity: ago(30) }),
    ]);
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).toBeTruthy();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-2", kind: "status" })).toBeUndefined();
  });
});

describe("keeping active branches warm", () => {
  it("syncs git status and the commit list for an active branch, run-scoped", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(App.call).toHaveBeenCalledWith("git.status", { run_id: "run-1" });
    expect(App.call).toHaveBeenCalledWith("git.log", { run_id: "run-1" });
    const log = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" });
    expect(log.value.commits).toHaveLength(1);
  });

  it("stores the status shape as received — there is no patch on it to strip", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    const status = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    expect(status.value.head).toBe("abc");
    expect(status.value.status_key).toBe(warmStatus().status_key);
    expect(status.value.files).toHaveLength(1);
  });

  it("prefetches the changed files' bodies in idle time, so expanding one is instant", async () => {
    const idle = [];
    globalThis.requestIdleCallback = (work) => idle.push(work);
    App.call = vi.fn(async (method, params) => {
      if (method === "git.status") return warmStatus();
      if (method === "git.diff") return warmTree.diff(params);
      if (method === "git.log") return { commits: [], more: false };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(App.call).not.toHaveBeenCalledWith("git.diff", expect.anything());

    idle.forEach((work) => work());
    await flush();
    expect(App.call).toHaveBeenCalledWith("git.diff", { run_id: "run-1", paths: ["src/a.js"] });
    const body = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "filediff", sub: "src/a.js" });
    expect(body.value.patch).toBe(patchFor("src/a.js", "new line"));
    delete globalThis.requestIdleCallback;
  });

  it("asks for no body it already holds, and never more than one call's worth", async () => {
    const many = Object.fromEntries(Array.from({ length: 60 }, (_unused, index) => [`f${index}.js`, "line"]));
    const big = worktreeOf(many);
    App.call = vi.fn(async (method, params) => {
      if (method === "git.status") return big.status();
      if (method === "git.diff") return big.diff(params);
      if (method === "git.log") return { commits: [], more: false };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    await flush();
    const asked = App.call.mock.calls.filter(([method]) => method === "git.diff");
    expect(asked).toHaveLength(1);
    expect(asked[0][1].paths).toHaveLength(50);
  });

  it("scopes a checkout Build does not own by project and worktree", async () => {
    sync.startCacheSync();
    await feed([branchItem({ run_id: null })]);
    expect(App.call).toHaveBeenCalledWith("git.status", { project_id: "p1", worktree_id: "wt-1" });
  });

  it("registers one change watcher per active branch and refreshes on delivery", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    const watcher = registeredWatchers.find((w) => w.entity === "run-1");
    expect(watcher).toBeTruthy();
    App.call.mockClear();
    watcher.refresh();
    await flush();
    expect(App.call).toHaveBeenCalledWith("git.status", { run_id: "run-1" });
  });

  it("lets a watcher go, disposed, when its entity leaves the active set", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    await feed([]);
    const watcher = registeredWatchers.find((w) => w.entity === "run-1");
    expect(watcher.disposed).toBe(true);
  });

  it("does not stack refreshes for an entity already being fetched", async () => {
    let settle;
    App.call = vi.fn(() => new Promise((resolve) => (settle = resolve)));
    sync.startCacheSync();
    await feed([branchItem()]);
    const watcher = registeredWatchers.find((w) => w.entity === "run-1");
    watcher.refresh();
    watcher.refresh();
    await flush();
    expect(App.call.mock.calls.length).toBe(2); // one status + one log, not four
    settle({});
  });

  it("asks nothing of git for an issue, but keeps its cache from eviction", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "iss-1", kind: "thread" }, {});
    sync.startCacheSync();
    await feed([
      { kind: "issue", project_id: "p2", issue_id: "iss-1", state: "plan_review", anchor: ago(2), last_activity: ago(2) },
    ]);
    expect(App.call).not.toHaveBeenCalledWith("git.status", expect.anything());
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "iss-1", kind: "thread" })).toBeTruthy();
  });
});

describe("one syncer per browser", () => {
  it("leaves the whole job to the tab that holds the lock", async () => {
    vi.stubGlobal("navigator", { locks: { request: vi.fn(async () => undefined) } }); // never granted
    vi.resetModules();
    cache = await import("../src/core/localCache.js");
    sync = await import("../src/core/cacheSync.js");
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(App.call).not.toHaveBeenCalled();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "", kind: "feed" })).toBeUndefined();
    vi.unstubAllGlobals();
  });
});

describe("the boot echo", () => {
  it("ignores the snapshot the cache itself painted", async () => {
    sync.startCacheSync();
    feedSubscriber({ ...snapshot([branchItem()]), cached: true });
    await flush();
    expect(App.call).not.toHaveBeenCalled();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "", kind: "feed" })).toBeUndefined();
  });
});

describe("keeping warmed conversations fresh", () => {
  it("re-reads a persisted branch thread on the entity's refresh", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 1 } }], deliveredSequence: 1 },
    );
    App.call = vi.fn(async (method) => {
      if (method === "git.status") return { head: "abc", patch: "p" };
      if (method === "git.log") return { commits: [] };
      if (method === "branch.get")
        return { run: { thread: { items: [{ id: "m-2", data: { sequence: 2 } }], has_more: false, thread_total: 2 } } };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(App.call).toHaveBeenCalledWith("branch.get", {
      project_id: "p1",
      branch: "build/login",
      agent_id: "ag-1",
      thread_limit: FIRST_PAGE_ITEMS,
    });
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" });
    expect(record.value.deliveredSequence).toBe(2);
  });

  it("re-reads an issue's persisted thread through issue.get", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "iss-1", kind: "thread", sub: "" },
      { items: [{ id: "m-1", data: { sequence: 1 } }], deliveredSequence: 1 },
    );
    App.call = vi.fn(async (method) =>
      method === "issue.get"
        ? { issue_id: "iss-1", thread: { items: [{ id: "m-3", data: { sequence: 3 } }], has_more: false, thread_total: 3 } }
        : {},
    );
    sync.startCacheSync();
    await feed([
      { kind: "issue", project_id: "p2", issue_id: "iss-1", state: "plan_review", anchor: ago(2), last_activity: ago(2) },
    ]);
    expect(App.call).toHaveBeenCalledWith("issue.get", { issue_id: "iss-1", thread_limit: FIRST_PAGE_ITEMS });
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "iss-1", kind: "thread", sub: "" });
    expect(record.value.deliveredSequence).toBe(3);
  });

  it("writes a surfaces record for each agent in the detail payload that carries one", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 1 } }], deliveredSequence: 1 },
    );
    App.call = vi.fn(async (method) => {
      if (method === "branch.get")
        return {
          run: {
            thread: { items: [{ id: "m-2", data: { sequence: 2 } }], has_more: false, thread_total: 2 },
            agents: [
              { id: "ag-1", surfaces: { shells: [{ id: "sh-1", description: "cargo test", state: "running" }] } },
              { id: "ag-2" },
            ],
          },
        };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "surfaces", sub: "ag-1" });
    expect(record.value.surfaces.shells).toHaveLength(1);
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "surfaces", sub: "ag-2" })).toBeUndefined();
  });

  it("writes the surfaces of an agent whose own conversation was never warmed", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 1 } }], deliveredSequence: 1 },
    );
    App.call = vi.fn(async (method) => {
      if (method === "branch.get")
        return {
          run: {
            thread: { items: [{ id: "m-2", data: { sequence: 2 } }], has_more: false, thread_total: 2 },
            agents: [
              { id: "ag-1", surfaces: { shells: [{ id: "sh-1", state: "running" }] } },
              { id: "ag-2", surfaces: { checklist: [{ id: "t-1", subject: "ship it", state: "pending" }] } },
            ],
          },
        };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "surfaces", sub: "ag-2" });
    expect(record.value.surfaces.checklist).toHaveLength(1);
  });

  it("rewrites nothing for a snapshot that stood still", async () => {
    const surfaces = { shells: [{ id: "sh-1", description: "cargo test", state: "running" }] };
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "surfaces", sub: "ag-1" };
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 1 } }], deliveredSequence: 1 },
    );
    const clock = vi.spyOn(Date, "now").mockReturnValue(1000);
    await cache.writeCached(address, { surfaces });
    clock.mockRestore();
    App.call = vi.fn(async (method) => {
      if (method === "branch.get")
        return {
          run: {
            thread: { items: [{ id: "m-2", data: { sequence: 2 } }], has_more: false, thread_total: 2 },
            agents: [{ id: "ag-1", surfaces }],
          },
        };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    expect((await cache.readCached(address)).at).toBe(1000);
  });

  it("asks for no conversation that was never opened", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(App.call).not.toHaveBeenCalledWith("branch.get", expect.anything());
  });
});

describe("keeping file listings warm", () => {
  it("syncs the top-level directory for an active branch", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(App.call).toHaveBeenCalledWith("fs.tree", { run_id: "run-1", path: "" });
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "" });
    expect(record).toBeTruthy();
  });

  it("re-lists the directories the reader walked into", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, { path: "src", entries: [] });
    App.call = vi.fn(async (method, params) => {
      if (method === "fs.tree") return { path: params.path, entries: [{ name: "fresh.js", kind: "file" }] };
      if (method === "git.status") return { head: "abc" };
      if (method === "git.log") return { commits: [] };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(App.call).toHaveBeenCalledWith("fs.tree", { run_id: "run-1", path: "src" });
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" });
    expect(record.value.entries).toHaveLength(1);
  });
});

describe("keeping a warmed review diff fresh", () => {
  it("re-reads run.diff only where the All-changes view was opened before", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(App.call).not.toHaveBeenCalledWith("run.diff", expect.anything());

    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: "old" });
    App.call.mockClear();
    const watcher = registeredWatchers.find((w) => w.entity === "run-1" && !w.disposed);
    App.call.mockImplementation(async (method) => {
      if (method === "run.diff") return { patch: "diff --git fresh" };
      if (method === "git.status") return { head: "abc" };
      if (method === "git.log") return { commits: [] };
      if (method === "fs.tree") return { path: "", entries: [] };
      return {};
    });
    watcher.refresh();
    await flush();
    expect(App.call).toHaveBeenCalledWith("run.diff", { run_id: "run-1" });
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" });
    expect(record.value.patch).toBe("diff --git fresh");
  });
});
