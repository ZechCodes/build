// The cache's sync layer: one tab holds the lock and follows the feed —
// persisting the snapshot, evicting entities the active set stops naming, and
// keeping every active branch's git status and commit list warm.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

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
  // settling on its own macrotask under fake-indexeddb.
  for (let i = 0; i < 25; i++) await new Promise((done) => setTimeout(done, 0));
};

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
    if (method === "git.status") return { head: "abc", patch: "diff --git a b", stat: { insertions: 1, deletions: 0 } };
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

  it("stores the status without the uncommitted patch — that loads on demand", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    const status = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    expect(status.value.head).toBe("abc");
    expect(status.value.patch).toBeUndefined();
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
