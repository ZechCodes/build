// The cache's sync layer: one tab holds the lock and follows the feed —
// persisting the snapshot, evicting entities the active set stops naming, and
// keeping every active branch's git status, commit list and file bodies warm.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { FIRST_PAGE_ITEMS } from "../src/core/thread.js";
import { patchFor, worktreeOf } from "./gitWireFixture.js";

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

/** Every read this layer makes is a warm-up, and rides the wire stamped so. */
const BACKGROUND = { priority: "background" };

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
// Everything in this file is the legacy contract: a bridge that serves no
// subscriptions, and the 60 s per-entity loop that is this layer's whole
// cadence there. The background tier has its own file.
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
  subscriptionsActive: () => false,
  onSubscriptionsChange: () => () => {},
}));

const App = {};
vi.mock("../src/app.js", () => ({ App }));

// The syncer works a device through its context, so this file registers them.
// A device's call is its own; the one this file mostly talks to answers through
// bridge.call, which every case scripts.
const contexts = new Map();
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: (deviceId) => contexts.get(deviceId) || null }));

const registerDevice = (deviceId, call = (...args) => bridge.call(...args)) => {
  const context = {
    deviceId,
    // The registry hands out the device's caller, not one session's.
    rpc: call,
    cacheScope: { deviceId, active: () => true },
    active: () => contexts.get(deviceId) === context,
  };
  contexts.set(deviceId, context);
  return context;
};

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

/** What subscribers get: the merge, with every device's own view beside it. */
const merged = (byDevice) => ({ ...snapshot(Object.values(byDevice).flatMap((view) => view.items)), devices: byDevice });

const feed = async (items) => {
  feedSubscriber(merged({ "dev-1": snapshot(items) }));
  await flush();
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete globalThis.navigator?.locks;
  registeredWatchers = [];
  feedSubscriber = null;
  contexts.clear();
  registerDevice("dev-1");
  bridge.call = vi.fn(async (method) => {
    if (method === "git.status") return warmStatus();
    if (method === "git.log") return { commits: [{ hash: "abc" }], more: false };
    return {};
  });
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/cacheSync.js");
});

afterEach(() => {
  sync.stopCacheSync();
  delete globalThis.requestIdleCallback;
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

// ---- more than one device --------------------------------------------------
// Every device answers for itself: its own snapshot, its own cache, its own
// eviction. Two machines both call their first project `proj-1` and can even
// name the same entity, so nothing about one device's rows may reach another's.
describe("following every device's feed", () => {
  const twoDevices = async (dev2Call) => {
    registerDevice("dev-2", dev2Call);
    const one = snapshot([branchItem()]);
    const two = snapshot([branchItem({ branch: "build/search" })]);
    sync.startCacheSync();
    feedSubscriber(merged({ "dev-1": one, "dev-2": two }));
    await flush();
  };

  it("persists each device's snapshot under its own device", async () => {
    await twoDevices(vi.fn(async () => ({})));
    const first = await cache.readCached({ deviceId: "dev-1", entityId: "", kind: "feed" });
    const second = await cache.readCached({ deviceId: "dev-2", entityId: "", kind: "feed" });
    expect(first.value.items[0].branch).toBe("build/login");
    expect(second.value.items[0].branch).toBe("build/search");
  });

  it("keys active rows by device and entity, so two devices' rows never collide", async () => {
    const secondCall = vi.fn(async (method) => {
      if (method === "git.status") return warmStatus();
      if (method === "git.log") return { commits: [{ hash: "def" }], more: false };
      return {};
    });
    await twoDevices(secondCall);
    // The same run id on two machines is two rows, each read through its own
    // device's call and written under its own device.
    expect(bridge.call).toHaveBeenCalledWith("git.status", { run_id: "run-1" }, BACKGROUND);
    expect(secondCall).toHaveBeenCalledWith("git.status", { run_id: "run-1" }, BACKGROUND);
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" })).toBeTruthy();
    expect((await cache.readCached({ deviceId: "dev-2", entityId: "run-1", kind: "log" })).value.commits[0].hash).toBe("def");
  });

  it("evicts within one device only what that device's view stops naming", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-2", kind: "status" }, {});
    await cache.writeCached({ deviceId: "dev-2", entityId: "run-2", kind: "status" }, {});
    registerDevice("dev-2", vi.fn(async () => ({})));
    const one = snapshot([branchItem()]);
    const two = snapshot([branchItem({ branch: "b2", run_id: "run-2", worktree_id: "wt-2" })]);
    sync.startCacheSync();
    feedSubscriber(merged({ "dev-1": one, "dev-2": two }));
    await flush();

    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-2", kind: "status" })).toBeUndefined();
    expect(await cache.readCached({ deviceId: "dev-2", entityId: "run-2", kind: "status" })).toBeTruthy();
  });

  it("lets a retired device's watchers go when it leaves the feed", async () => {
    registerDevice("dev-2", vi.fn(async () => ({})));
    const one = snapshot([branchItem()]);
    const two = snapshot([branchItem({ branch: "b2", run_id: "run-2", worktree_id: "wt-2" })]);
    sync.startCacheSync();
    feedSubscriber(merged({ "dev-1": one, "dev-2": two }));
    await flush();

    // dev-2 is retired: it is no longer in the merge at all.
    feedSubscriber(merged({ "dev-1": one }));
    await flush();
    expect(registeredWatchers.find((watcher) => watcher.entity === "run-2").disposed).toBe(true);
    expect(registeredWatchers.find((watcher) => watcher.entity === "run-1").disposed).toBe(false);
  });

  it("leaves a device with no context alone — nothing can be read for it", async () => {
    const one = snapshot([branchItem()]);
    sync.startCacheSync();
    feedSubscriber(merged({ "dev-9": one }));
    await flush();
    expect(await cache.readCached({ deviceId: "dev-9", entityId: "", kind: "feed" })).toBeUndefined();
  });
});

describe("keeping active branches warm", () => {
  it("conditionally refreshes a cached status and keeps the full held shape on an unchanged answer", async () => {
    const held = warmStatus();
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, held);
    bridge.call = vi.fn(async (method, params) => {
      if (method === "git.status") return { unchanged: true, status_key: held.status_key };
      if (method === "git.log") return { commits: [], more: false };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(bridge.call).toHaveBeenCalledWith("git.status", { run_id: "run-1", if_status_key: held.status_key }, BACKGROUND);
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).value.files).toEqual(held.files);
  });

  it("syncs git status and the commit list for an active branch, run-scoped", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(bridge.call).toHaveBeenCalledWith("git.status", { run_id: "run-1" }, BACKGROUND);
    expect(bridge.call).toHaveBeenCalledWith("git.log", { run_id: "run-1" }, BACKGROUND);
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
    bridge.call = vi.fn(async (method, params) => {
      if (method === "git.status") return warmStatus();
      if (method === "git.diff") return warmTree.diff(params);
      if (method === "git.log") return { commits: [], more: false };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(bridge.call).not.toHaveBeenCalledWith("git.diff", expect.anything(), expect.anything());

    idle.forEach((work) => work());
    await flush();
    // A warm-up rides the background queue, and says so on the envelope.
    expect(bridge.call).toHaveBeenCalledWith("git.diff", { run_id: "run-1", paths: ["src/a.js"] }, { priority: "background" });
    const body = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "filediff", sub: "src/a.js" });
    expect(body.value.patch).toBe(patchFor("src/a.js", "new line"));
  });

  it("keeps syncing an entity whose idle turn never comes", async () => {
    const idleNeverRun = [];
    globalThis.requestIdleCallback = (work) => idleNeverRun.push(work);
    bridge.call = vi.fn(async (method, params) => {
      if (method === "git.status") return warmStatus();
      if (method === "git.diff") return warmTree.diff(params);
      if (method === "git.log") return { commits: [], more: false };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(idleNeverRun.length).toBeGreaterThan(0);

    bridge.call.mockClear();
    registeredWatchers.find((watcher) => watcher.entity === "run-1").refresh();
    await flush();
    expect(bridge.call).toHaveBeenCalledWith("git.status", { run_id: "run-1", if_status_key: warmStatus().status_key }, BACKGROUND);
  });

  it("warms only the leading viewport budget instead of every offscreen body", async () => {
    const many = Object.fromEntries(Array.from({ length: 60 }, (_unused, index) => [`f${index}.js`, "line"]));
    const big = worktreeOf(many);
    bridge.call = vi.fn(async (method, params) => {
      if (method === "git.status") return big.status();
      if (method === "git.diff") return big.diff(params);
      if (method === "git.log") return { commits: [], more: false };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    await flush();
    const asked = bridge.call.mock.calls.filter(([method]) => method === "git.diff");
    expect(asked).toHaveLength(1);
    expect(asked[0][1].paths).toHaveLength(3);
  });

  it("scopes a checkout Build does not own by project and worktree", async () => {
    sync.startCacheSync();
    await feed([branchItem({ run_id: null })]);
    expect(bridge.call).toHaveBeenCalledWith("git.status", { project_id: "p1", worktree_id: "wt-1" }, BACKGROUND);
  });

  it("registers one change watcher per active branch and refreshes on delivery", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    const watcher = registeredWatchers.find((w) => w.entity === "run-1");
    expect(watcher).toBeTruthy();
    bridge.call.mockClear();
    watcher.refresh();
    await flush();
    expect(bridge.call).toHaveBeenCalledWith("git.status", { run_id: "run-1", if_status_key: warmStatus().status_key }, BACKGROUND);
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
    bridge.call = vi.fn(() => new Promise((resolve) => (settle = resolve)));
    sync.startCacheSync();
    await feed([branchItem()]);
    const watcher = registeredWatchers.find((w) => w.entity === "run-1");
    watcher.refresh();
    watcher.refresh();
    await flush();
    expect(bridge.call.mock.calls.length).toBe(2); // one status + one log, not four
    settle({});
  });

  it("asks nothing of git for an issue, but keeps its cache from eviction", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "iss-1", kind: "thread" }, {});
    sync.startCacheSync();
    await feed([
      { kind: "issue", project_id: "p2", issue_id: "iss-1", state: "plan_review", anchor: ago(2), last_activity: ago(2) },
    ]);
    expect(bridge.call).not.toHaveBeenCalledWith("git.status", expect.anything());
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
    expect(bridge.call).not.toHaveBeenCalled();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "", kind: "feed" })).toBeUndefined();
    vi.unstubAllGlobals();
  });
});

describe("the boot echo", () => {
  it("ignores the snapshot the cache itself painted", async () => {
    sync.startCacheSync();
    const view = { ...snapshot([branchItem()]), cached: true };
    feedSubscriber({ ...merged({ "dev-1": view }), cached: true });
    await flush();
    expect(bridge.call).not.toHaveBeenCalled();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "", kind: "feed" })).toBeUndefined();
  });
});

describe("keeping warmed conversations fresh", () => {
  it("re-reads a persisted branch thread on the entity's refresh", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 1 } }], deliveredSequence: 1 },
    );
    bridge.call = vi.fn(async (method) => {
      if (method === "git.status") return { head: "abc", patch: "p" };
      if (method === "git.log") return { commits: [] };
      if (method === "branch.get")
        return { run: { thread: { items: [{ id: "m-2", data: { sequence: 2 } }], has_more: false, thread_total: 2 } } };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(bridge.call).toHaveBeenCalledWith(
      "branch.get",
      { project_id: "p1", branch: "build/login", agent_id: "ag-1", thread_limit: FIRST_PAGE_ITEMS },
      BACKGROUND,
    );
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" });
    expect(record.value.deliveredSequence).toBe(2);
  });

  it("re-reads an issue's persisted thread through issue.get", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "iss-1", kind: "thread", sub: "" },
      { items: [{ id: "m-1", data: { sequence: 1 } }], deliveredSequence: 1 },
    );
    bridge.call = vi.fn(async (method) =>
      method === "issue.get"
        ? { issue_id: "iss-1", thread: { items: [{ id: "m-3", data: { sequence: 3 } }], has_more: false, thread_total: 3 } }
        : {},
    );
    sync.startCacheSync();
    await feed([
      { kind: "issue", project_id: "p2", issue_id: "iss-1", state: "plan_review", anchor: ago(2), last_activity: ago(2) },
    ]);
    expect(bridge.call).toHaveBeenCalledWith("issue.get", { issue_id: "iss-1", thread_limit: FIRST_PAGE_ITEMS }, BACKGROUND);
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "iss-1", kind: "thread", sub: "" });
    expect(record.value.deliveredSequence).toBe(3);
  });

  it("writes a surfaces record for each agent in the detail payload that carries one", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 1 } }], deliveredSequence: 1 },
    );
    bridge.call = vi.fn(async (method) => {
      if (method === "branch.get")
        return {
          run: {
            thread: { items: [{ id: "m-2", data: { sequence: 2 } }], has_more: false, thread_total: 2 },
            agents: [
              { id: "ag-1", surface_session_generation: "session-1", surfaces: { shells: [{ id: "sh-1", description: "cargo test", state: "running" }] } },
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
    bridge.call = vi.fn(async (method) => {
      if (method === "branch.get")
        return {
          run: {
            thread: { items: [{ id: "m-2", data: { sequence: 2 } }], has_more: false, thread_total: 2 },
            agents: [
              { id: "ag-1", surface_session_generation: "session-1", surfaces: { shells: [{ id: "sh-1", state: "running" }] } },
              { id: "ag-2", surface_session_generation: "session-2", surfaces: { checklist: [{ id: "t-1", subject: "ship it", state: "pending" }] } },
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
    await cache.writeCached(address, { surfaces, generation: "session-1" });
    clock.mockRestore();
    bridge.call = vi.fn(async (method) => {
      if (method === "branch.get")
        return {
          run: {
            thread: { items: [{ id: "m-2", data: { sequence: 2 } }], has_more: false, thread_total: 2 },
            agents: [{ id: "ag-1", surface_session_generation: "session-1", surfaces }],
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
    expect(bridge.call).not.toHaveBeenCalledWith("branch.get", expect.anything());
  });
});

describe("keeping file listings warm", () => {
  it("syncs the top-level directory for an active branch", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(bridge.call).toHaveBeenCalledWith("fs.tree", { run_id: "run-1", path: "" }, BACKGROUND);
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "" });
    expect(record).toBeTruthy();
  });

  it("re-lists the directories the reader walked into", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, { path: "src", entries: [] });
    bridge.call = vi.fn(async (method, params) => {
      if (method === "fs.tree") return { path: params.path, entries: [{ name: "fresh.js", kind: "file" }] };
      if (method === "git.status") return { head: "abc" };
      if (method === "git.log") return { commits: [] };
      return {};
    });
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(bridge.call).toHaveBeenCalledWith("fs.tree", { run_id: "run-1", path: "src" }, BACKGROUND);
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" });
    expect(record.value.entries).toHaveLength(1);
  });
});

describe("keeping a warmed review diff fresh", () => {
  it("re-reads run.diff only where the All-changes view was opened before", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(bridge.call).not.toHaveBeenCalledWith("run.diff", expect.anything());

    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: "old" });
    bridge.call.mockClear();
    const watcher = registeredWatchers.find((w) => w.entity === "run-1" && !w.disposed);
    bridge.call.mockImplementation(async (method) => {
      if (method === "run.diff") return { patch: "diff --git fresh" };
      if (method === "git.status") return { head: "abc" };
      if (method === "git.log") return { commits: [] };
      if (method === "fs.tree") return { path: "", entries: [] };
      return {};
    });
    watcher.refresh();
    await flush();
    expect(bridge.call).toHaveBeenCalledWith("run.diff", { run_id: "run-1" }, BACKGROUND);
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" });
    expect(record.value.patch).toBe("diff --git fresh");
  });
});
