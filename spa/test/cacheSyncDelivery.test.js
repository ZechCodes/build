// @vitest-environment jsdom
// The wire, end to end: what the bridge flushes at the subscriptions this
// module takes out is what this module applies.
//
// Every other test of the sync layer stands in for `changeEvents` so it can
// hand a flush straight to a registration. That proves what the appliers write
// and nothing about the route between the two. This file mocks nothing on that
// route: it registers the real subscriptions, dispatches a real `changes`
// frame at the real router, and reads the cache afterwards.

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

const bridge = { call: null };

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/app.js", () => ({ App }));

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

const registerDevice = (deviceId) => {
  const context = {
    deviceId,
    rpc: (...asked) => bridge.call(...asked),
    session: { device: deviceId },
    greeted: Promise.resolve(),
    cacheScope: { deviceId, active: () => true },
    active: () => contexts.get(deviceId) === context,
  };
  contexts.set(deviceId, context);
  return context;
};

let cache, sync, changeEvents;
let board = [];

const ANSWERS = {
  "board.list": () => ({ items: board }),
  "project.list": () => ({ projects: [{ project_id: "p1", name: "build" }] }),
  "workspace.list": () => ({ workspaces: [] }),
  "git.status": () => ({ head: "abc", status_key: "key-1", files: [] }),
  "git.log": () => ({ commits: [{ hash: "c1" }], newest: "c1" }),
  "git.unpushed": () => ({ base: {}, commits: [], diff_key: "d1" }),
  "term.list": () => ({ terminals: [] }),
  "fs.tree": (params) => ({ path: params.path, entries: [] }),
  "thread.page": () => ({ items: [], has_more: false }),
  "run.diff": () => ({ patch: "pulled", diff_key: "d2" }),
};

const answer = (method, params) => (ANSWERS[method] || (() => ({})))(params || {});

const settle = async () => {
  let before = -1;
  while (before !== bridge.call.mock.calls.length) {
    before = bridge.call.mock.calls.length;
    for (let turn = 0; turn < 12; turn += 1) await new Promise((done) => setTimeout(done, 0));
  }
};

/** The route that stands on the one branch the board lists. */
const BRANCH_ROUTE = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" };

/** The sync layer up, its subscriptions registered, and a bridge that pushes. */
const boot = async (items = [branchItem()], route = { name: "inbox" }) => {
  board = items;
  App.route = route;
  sync.startCacheSync();
  await settle();
  changeEvents.armChangeEvents({ push_events: true }, "dev-1");
};

/** One flush off the wire, at the device that sent it. */
const flush = async (items, subscriptionId = "s-inbox") => {
  changeEvents.dispatchChangeEvent({ type: "changes", subscription_id: subscriptionId, items }, "dev-1");
  await settle();
};

const calls = (method) => bridge.call.mock.calls.filter(([name]) => name === method);
const read = (entityId, kind, sub = "") => cache.readCached({ deviceId: "dev-1", entityId, kind, sub });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete globalThis.navigator?.locks;
  stateListeners.clear();
  contexts.clear();
  board = [];
  App.route = { name: "inbox" };
  registerDevice("dev-1");
  bridge.call = vi.fn(async (method, params) => answer(method, params));
  cache = await import("../src/core/localCache.js");
  changeEvents = await import("../src/core/changeEvents.js");
  sync = await import("../src/core/cacheSync.js");
});

afterEach(() => {
  sync.stopCacheSync();
  changeEvents.resetChangeEvents();
});

describe("a flush arriving at the real subscriptions", () => {
  it("writes the row a state item carries", async () => {
    await boot();
    await flush([{ entity_id: "run-1", state: { ...branchItem(), state: "review" } }]);
    expect((await read("run-1", "row")).value.state).toBe("review");
  });

  it("lets go of the data of an entity the board item says left", async () => {
    await boot();
    expect(await read("run-1", "status")).toBeTruthy();
    await flush([{ entity_id: "board", state: { revision: 4, removed: ["run-1"] } }]);
    expect(await read("run-1", "status")).toBeUndefined();
  });

  // The rail is the `feed` record's list with each row's own record laid over
  // it (core/taskFeed.js), so a row survives in it until BOTH are gone. A
  // branch deleted on another machine is named in `removed` and in no `state`
  // item — there is no finished state to report for something that is not
  // there — so the push is the only word this tab gets.
  it("takes the row off the rail when the board item says the entity left", async () => {
    await boot();
    expect((await read("run-1", "row")).value).toBeTruthy();
    await flush([{ entity_id: "board", state: { revision: 4, removed: ["run-1"] } }]);
    expect(await read("run-1", "row")).toBeUndefined();
    expect((await read("", "feed")).value.items).toEqual([]);
  });

  it("applies it once, though all three subscriptions cover the workspace", async () => {
    await boot([branchItem()], BRANCH_ROUTE);
    bridge.call.mockClear();
    await flush(
      [{ entity_id: "run-1", git: { log: { commits: [{ hash: "h7", ahead_of_base: true }], newest: "h7" } } }],
      "s-active:run-1",
    );
    expect(calls("git.show")).toHaveLength(1);
  });

  it("re-lists a walked directory once for one flush", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, { path: "src", entries: [] });
    await boot([branchItem()], BRANCH_ROUTE);
    bridge.call.mockClear();
    await flush(
      [{ entity_id: "run-1", files: { paths: ["src/a.js"], truncated: false, root: { path: "", entries: [] } } }],
      "s-background",
    );
    expect(calls("fs.tree").filter(([, params]) => params.path === "src")).toHaveLength(1);
  });

  // Issue #58: the harnesses out of usage on the device, from the whole read
  // and from the board item that says the list moved — through the real sync
  // path, so a key read from the wrong place fails here.
  it("holds the usage limits the board lists, and the ones a board item carries", async () => {
    const limit = {
      harness: "claude_adk",
      since: "2026-09-20T21:30:47Z",
      resets_at: "2026-09-20T22:20:00Z",
      said: "You've hit your session limit · resets 6:20pm (America/New_York)",
    };
    bridge.call = vi.fn(async (method, params) =>
      method === "board.list" ? { items: board, usage_limits: [limit] } : answer(method, params));
    await boot();
    const limits = await import("../src/core/usageLimits.js");
    expect(limits.usageLimitsOf("dev-1")).toEqual([limit]);

    await flush([{ entity_id: "board", state: { revision: 6, removed: ["run-9"] } }]);
    expect(limits.usageLimitsOf("dev-1")).toEqual([limit]);

    await flush([{ entity_id: "board", state: { revision: 7, usage_limits: [] } }]);
    expect(limits.usageLimitsOf("dev-1")).toEqual([]);
  });

  it("writes the lists a board item carries", async () => {
    await boot();
    await flush([{ entity_id: "board", state: { revision: 5, projects: [{ project_id: "p2", name: "relaydb" }] } }]);
    expect((await read("", "projects")).value).toEqual([
      expect.objectContaining({ id: "p2", deviceId: "dev-1" }),
    ]);
  });
});
