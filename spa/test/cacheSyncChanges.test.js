// @vitest-environment jsdom
// The cache's background tier (wire spec step 1.6): two all-scope
// subscriptions carry what moved, and the syncer pulls only what its own
// records disagree with — at background priority, with a ten-minute sweep and
// one on visibilitychange behind them instead of the old 60 s loop.
//
// The syncer works a machine through its context, so every case here adopts a
// session for one: the answers are that device's, and the records they land in
// are keyed by it.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { worktreeOf } from "./gitWireFixture.js";
import { sessionAnswering } from "./deviceSessionFixture.js";
import { liveFeedSnapshot, mergeFeeds } from "../src/core/feedMerge.js";

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
  dropFeedDevice: () => {},
}));

let registeredWatchers = [];
let subscribing = true;
const modeListeners = new Set();
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
  subscriptionsActive: () => subscribing,
  onSubscriptionsChange: (fn) => {
    modeListeners.add(fn);
    return () => modeListeners.delete(fn);
  },
  disarmChangeEvents: () => {},
}));

const App = { devices: [{ id: "dev-1", name: "laptop", status: "online" }], viewingContext: null };
vi.mock("../src/app.js", () => ({ App }));

/** The one machine this file talks to. Its session answers with whatever
 *  `bridge.call` is standing at the time, so a case that re-scripts it is that
 *  bridge answering differently, not another device. */
const bridge = { call: null };

/** Every read this layer makes is a warm-up, and rides the wire stamped so. */
const BACKGROUND = { priority: "background" };

let cache, sync, devices;

const flush = async () => {
  for (let i = 0; i < 40; i++) await new Promise((done) => setTimeout(done, 0));
};

const warmTree = worktreeOf({ "src/a.js": "new line" });
const warmStatus = () => warmTree.status({ head: "abc", stat: { insertions: 1, deletions: 0 } });

/** The account's snapshot as subscribers get it: one device's reading of its
 *  own wire — rows stamped with it — inside the merge. */
const feed = async (items, deviceId = "dev-1") => {
  const view = liveFeedSnapshot({ items }, {}, {}, deviceId);
  feedSubscriber(mergeFeeds(new Map([[deviceId, view]]), [deviceId]));
  await flush();
};

const background = () => registeredWatchers.filter((watcher) => watcher.scope === "all" && !watcher.disposed);
const entityWatchers = () => registeredWatchers.filter((watcher) => watcher.entity && !watcher.disposed);

const deliver = async (items) => {
  for (const watcher of background()) watcher.onChanges(items);
  await flush();
};

const called = (method) => bridge.call.mock.calls.filter(([name]) => name === method);

/** The methods this layer asked for without the background stamp: nothing, on
 *  every path, since nothing here is what a reader is waiting on. */
const readsAheadOfTheForeground = () =>
  bridge.call.mock.calls.filter(([, , envelope]) => envelope?.priority !== "background").map(([method]) => method);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete globalThis.navigator?.locks;
  registeredWatchers = [];
  modeListeners.clear();
  subscribing = true;
  feedSubscriber = null;
  bridge.call = vi.fn(async (method) => {
    if (method === "git.status") return warmStatus();
    if (method === "git.log") return { commits: [{ hash: "abc" }], more: false };
    if (method === "fs.tree") return { path: "", entries: [] };
    return {};
  });
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/cacheSync.js");
  devices = await import("../src/core/deviceContexts.js");
  devices.adoptDeviceSession(sessionAnswering(bridge, "dev-1"));
});

afterEach(() => {
  sync.stopCacheSync();
  devices.resetDeviceContexts();
});

describe("the background tier's two subscriptions", () => {
  it("asks for state, thread and git every 30 s and files every 3 minutes, both background", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(background().map((w) => [w.deviceId, w.kinds, w.mode, w.priority])).toEqual([
      ["dev-1", ["state", "thread", "git"], { batch_ms: 30000 }, "background"],
      ["dev-1", ["files"], { batch_ms: 180000 }, "background"],
    ]);
  });

  it("puts the safety sweep at ten minutes, not the old 60 s loop", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(background()[0].intervalMs).toBe(600000);
  });

  it("keeps no per-entity poll while subscriptions carry the board", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(entityWatchers()).toEqual([]);
  });

  it("still warms an entity the moment it enters the active set, and only behind the foreground", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    expect(called("git.status")).toHaveLength(1);
    expect(readsAheadOfTheForeground()).toEqual([]);
  });

  it("brings the 60 s per-entity poll back on a bridge that fell to legacy", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    subscribing = false;
    modeListeners.forEach((fn) => fn("dev-1", false));
    await flush();
    expect(entityWatchers().map((w) => [w.entity, w.deviceId, w.intervalMs])).toEqual([["run-1", "dev-1", 60000]]);
  });
});

describe("pulling only on a key miss", () => {
  it("asks git for nothing when the pushed status key is the one it holds", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", git: { status_key: warmStatus().status_key, head: "abc" } }]);
    expect(called("git.status")).toEqual([]);
  });

  it("re-reads status and the log at background priority when the key moved", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", git: { status_key: "9f3c1a0b7e2d4c55", head: "def" } }]);
    expect(called("git.status")[0]).toEqual([
      "git.status",
      { run_id: "run-1", if_status_key: warmStatus().status_key },
      BACKGROUND,
    ]);
    expect(called("git.log")).toHaveLength(1);
    expect(readsAheadOfTheForeground()).toEqual([]);
  });

  it("re-reads a conversation only when the pushed sequence is past the cached one", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 7 } }], deliveredSequence: 7 },
    );
    sync.startCacheSync();
    await feed([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", thread: [{ agent_id: "ag-1", last_sequence: 7 }] }]);
    expect(called("branch.get")).toEqual([]);

    await deliver([{ entity_id: "run-1", thread: [{ agent_id: "ag-1", last_sequence: 9 }] }]);
    expect(called("branch.get")).toHaveLength(1);
    expect(readsAheadOfTheForeground()).toEqual([]);
  });

  it("asks for nothing when the pushed state is what the feed row already says", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [], deliveredSequence: 1 },
    );
    sync.startCacheSync();
    await feed([branchItem({ state: "building", agents: [{ id: "ag-1" }], unread_reason: null })]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", state: { run: "building", agents: 1, attention: "none" } }]);
    expect(called("branch.get")).toEqual([]);
  });

  it("re-reads the entity's detail when the pushed state disagrees with the row", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [], deliveredSequence: 1 },
    );
    sync.startCacheSync();
    await feed([branchItem({ state: "building", agents: [{ id: "ag-1" }], unread_reason: null })]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", state: { run: "building", agents: 1, attention: "agent_message" } }]);
    expect(called("branch.get")).toHaveLength(1);
  });

  it("re-reads the entity's detail when its state moved", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [], deliveredSequence: 1 },
    );
    sync.startCacheSync();
    await feed([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", state: {} }]);
    expect(called("branch.get")).toHaveLength(1);
  });

  it("re-lists the directories the changed paths sit in, and every one on a truncated list", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, { path: "src", entries: [] });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "docs" }, { path: "docs", entries: [] });
    sync.startCacheSync();
    await feed([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", files: { paths: ["src/a.js"], truncated: false } }]);
    expect(called("fs.tree").map(([, params]) => params.path).sort()).toEqual(["", "src"]);

    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", files: { paths: [], truncated: true } }]);
    expect(called("fs.tree").map(([, params]) => params.path).sort()).toEqual(["", "docs", "src"]);
    expect(readsAheadOfTheForeground()).toEqual([]);
  });

  it("ignores an item for an entity the active set does not name", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-404", git: { status_key: "moved" } }]);
    expect(bridge.call).not.toHaveBeenCalled();
  });
});

describe("the safety sweep", () => {
  it("re-reads every active entity on the ten-minute pass", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    bridge.call.mockClear();
    background()[0].refresh();
    await flush();
    expect(called("git.status")).toHaveLength(1);
    expect(readsAheadOfTheForeground()).toEqual([]);
  });

  it("sweeps once when the tab comes back", async () => {
    sync.startCacheSync();
    await feed([branchItem()]);
    bridge.call.mockClear();
    document.dispatchEvent(new Event("visibilitychange"));
    await flush();
    expect(called("git.status")).toHaveLength(1);
  });
});
