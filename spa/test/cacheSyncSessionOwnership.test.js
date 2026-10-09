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
  task_id: null,
  agents: [],
  ...over,
});

/** A project's own conversation, as the board carries it: an ordinary row over
 *  the scratch checkout the bridge cut for it, whose agent is a project agent.
 *  Quiet for days, which is the ordinary state of a project agent — nobody
 *  talks to one every day. */
const projectRow = (over = {}) =>
  branchItem({
    branch: "build",
    run_id: "run-proj",
    worktree_id: "wt-proj",
    state: "review",
    anchor: ago(80),
    last_activity: ago(80),
    agents: [{ id: "project-01M2" }],
    ...over,
  });

/** The `project.list` row that names it. `entity_id` is the conversation the
 *  project holds — null for a project nobody has talked to yet. */
const projectList = { project_id: "p1", name: "build", entity_id: "run-proj", run_id: "run-proj" };

/** The one bridge this file's device answers through. */
const bridge = { call: null };

/** Where the reader is standing. The routed workspace leads every pass and is
 *  the only one read in front of the foreground. */
const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/appState.js", async () => ({ App: (await import("../src/app.js")).App }));
vi.mock("../src/app.js", () => ({ App }));

let registeredWatchers = [];
const subscriptionHeldListeners = new Set();
vi.mock("../src/core/changeEvents.js", () => ({
  // The greeting says which kinds a bridge carries; a stand-in that
  // answers none would have the sync layer ask for none of the new ones.
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] } }),
  // A stand-in bridge holds whatever it is asked to at once
  // (test/cacheSyncDelivery.test.js drives the real one).
  subscriptionsSettledFor: async () => {},
  onSubscriptionHeld: (fn) => {
    subscriptionHeldListeners.add(fn);
    return () => subscriptionHeldListeners.delete(fn);
  },
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
  "tasks.list": () => ({ tasks: [] }),
  "tasks.columns": () => ({ project_id: "p1", columns: [{ id: "backlog", name: "Backlog" }] }),
  "run.diff": () => ({ patch: "the whole diff", diff_key: "d1" }),
  "worktree.diff": () => ({ patch: "the whole diff", diff_key: "d1" }),
};

const answer = (method, params) => (script[method] || ANSWERS[method] || (() => ({})))(params || {});

const calls = (method) => bridge.call.mock.calls.filter(([name]) => name === method);
const paramsOf = (method) => calls(method).map(([, params]) => params);
const read = (entityId, kind, sub = "") => cache.readCached({ deviceId: "dev-1", entityId, kind, sub });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete globalThis.navigator?.locks;
  globalThis.requestIdleCallback = (callback) => queueMicrotask(callback);
  registeredWatchers = [];
  subscriptionHeldListeners.clear();
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


const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
function holdNext(method, value) {
  const reading = deferred();
  const answer = deferred();
  script[method] = () => {
    delete script[method];
    reading.resolve();
    return answer.promise;
  };
  return { reading: reading.promise, release: () => answer.resolve(value) };
}
const announce = () => { for (const listener of stateListeners) listener(); };

it("review 464: canceled automatic waiter leaves the new lifetime marked synced", async () => {
  const held = holdNext("board.list", { items: [] });
  sync.startCacheSync();
  const first = sync.passInFlight("dev-1");
  await held.reading;
  registerDevice("dev-1");
  announce(); // automatic B syncSessionOnce waits for A
  const observedWaiter = sync.syncDevice("dev-1");
  sync.stopCacheSync();
  sync.startCacheSync();
  expect(await sync.passInFlight("dev-1")).toBe(true);
  held.release();
  await Promise.all([first, observedWaiter]);
  // Drain any pass a baseline implementation incorrectly starts on release;
  // this probe isolates the later ordinary announcement's extra traffic.
  while (sync.passInFlight("dev-1")) await sync.passInFlight("dev-1");
  const beforeAnnouncement = bridge.call.mock.calls.slice();
  announce(); // ordinary device/transport announcement, same B session
  const unexpected = sync.passInFlight("dev-1");
  if (unexpected) await unexpected;
  expect(bridge.call.mock.calls).toEqual(beforeAnnouncement);
  expect(unexpected).toBeNull();
});

it("review 464: same-session restart preserves completed automatic-sync ownership", async () => {
  const held = holdNext("board.list", { items: [] });
  sync.startCacheSync();
  const first = sync.passInFlight("dev-1");
  await held.reading;
  const queued = sync.syncDevice("dev-1");
  sync.stopCacheSync();
  sync.startCacheSync();
  expect(await sync.passInFlight("dev-1")).toBe(true);
  held.release();
  expect(await Promise.all([first, queued])).toEqual([false, false]);
  const before = bridge.call.mock.calls.slice();
  announce();
  const unexpected = sync.passInFlight("dev-1");
  if (unexpected) await unexpected;
  expect(bridge.call.mock.calls).toEqual(before);
  expect(unexpected).toBeNull();
});

