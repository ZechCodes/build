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
vi.mock("../src/app.js", () => ({ App }));

let registeredWatchers = [];
vi.mock("../src/core/changeEvents.js", () => ({
  // The greeting says which kinds a bridge carries; a stand-in that
  // answers none would have the sync layer ask for none of the new ones.
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "issues"] } }),
  // A stand-in bridge holds whatever it is asked to at once
  // (test/cacheSyncDelivery.test.js drives the real one).
  subscriptionsSettledFor: async () => {},
  onSubscriptionHeld: () => () => {},
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
  "issues.list": () => ({ issues: [] }),
  "issues.columns": () => ({ project_id: "p1", columns: [{ id: "backlog", name: "Backlog" }] }),
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

  it("reads one workspace's shapes in the order a reader opening it wants them", async () => {
    // What is on screen first is read first. The conversation comes before the
    // patches behind the unpushed commits, which are twenty reads of a quarter
    // of a megabyte and nothing anybody is looking at yet — and the projects'
    // issue lists come after every workspace, because an issue list is a
    // project surface and the inbox is the landing one.
    script["git.log"] = () => ({ commits: [{ hash: "h1", ahead_of_base: true }], newest: "h1", reset: false });
    await boot([branchItem({ agents: [{ id: "ag-1" }] })], routeTo("build/login"));
    expect(bridge.call.mock.calls.map(([method]) => method).slice(3)).toEqual([
      "git.status",
      "fs.tree",
      "term.list",
      "git.log",
      "git.unpushed",
      "thread.page",
      "git.show",
      "run.diff",
      "issues.list",
      "issues.columns",
    ]);
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

  // The patch is not asked for at all now: the record never held it, and on a
  // phone's relayed path it was most of a megabyte per cold pass.
  it("asks for the unpushed shape without the patch that weighs it", async () => {
    await boot([branchItem()]);
    expect(paramsOf("git.unpushed")).toEqual([{ run_id: "run-1", patch: false }]);
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

  it("keeps how far back the history reaches when a forward page walks nothing", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "log" },
      { commits: [{ hash: "c0" }], newest: "c0", more: true },
    );
    script["git.log"] = () => ({ commits: [], newest: "c0", more: false, reset: false });
    await boot([branchItem()]);
    const record = (await read("run-1", "log")).value;
    expect(record.commits.map((one) => one.hash)).toEqual(["c0"]);
    // How far back the history reaches is not what a forward page answers: it
    // walked from the cursor to HEAD and never reached the end of the list the
    // record is holding. Losing this is the "Load older commits\u2026" affordance.
    expect(record.more).toBe(true);
  });

  it("takes the answer's own reach where the record's commits are gone", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "log" },
      { commits: [{ hash: "c0" }], newest: "c0", more: false },
    );
    script["git.log"] = () => ({ commits: [{ hash: "r1" }], newest: "r1", more: true, reset: true });
    await boot([branchItem()]);
    expect((await read("run-1", "log")).value.more).toBe(true);
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

  it("lets the cursor go when a reset answer carries no commits at all", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, { commits: [{ hash: "c0" }], newest: "c0" });
    script["git.log"] = () => ({ commits: [], reset: true });
    await boot([branchItem()]);
    const record = (await read("run-1", "log")).value;
    expect(record.commits).toEqual([]);
    // Keeping the pre-reset hash would have the next pass ask after a commit
    // this checkout no longer has — which answers `reset` again, for ever.
    expect(record.newest).toBeNull();
  });

  it("reads a conversation forward from the sequence it holds", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 7 } }], deliveredSequence: 7, olderItemsRemain: true },
    );
    script["thread.page"] = () => ({ items: [{ id: "m-2", data: { sequence: 8 } }], has_more: false });
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    expect(paramsOf("thread.page")).toEqual([
      { entity_id: "run-1", agent_id: "ag-1", after_sequence: 7, newest: true, limit: sync.LATEST_THREAD_ITEMS },
    ]);
    const record = await read("run-1", "thread", "ag-1");
    expect(record.value.items.map((one) => one.id)).toEqual(["m-1", "m-2"]);
    expect(record.value.deliveredSequence).toBe(8);
    // How far back the window reaches is not what a forward page answers.
    expect(record.value.olderItemsRemain).toBe(true);
  });

  it("replaces a stale warm window with only the newest hundred after a gap", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-7", data: { sequence: 7 } }], deliveredSequence: 7, olderItemsRemain: false },
    );
    const items = Array.from({ length: 100 }, (_, index) => ({
      id: `m-${index + 58}`,
      data: { sequence: index + 58 },
    }));
    script["thread.page"] = () => ({
      items,
      has_more: true,
      thread_total: 157,
      thread_last_sequence: 157,
    });

    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);

    expect(paramsOf("thread.page")).toEqual([
      { entity_id: "run-1", agent_id: "ag-1", after_sequence: 7, newest: true, limit: sync.LATEST_THREAD_ITEMS },
    ]);
    const record = await read("run-1", "thread", "ag-1");
    expect(record.value.items.map((one) => one.data.sequence)).toEqual(
      Array.from({ length: 100 }, (_, index) => index + 58),
    );
    expect(record.value.deliveredSequence).toBe(157);
    expect(record.value.knownTotalItems).toBe(157);
    expect(record.value.olderItemsRemain).toBe(true);
  });

  it("keeps the legacy forward-page append path when newest is omitted", () => {
    const held = {
      items: [{ id: "m-7", data: { sequence: 7 } }],
      deliveredSequence: 7,
      olderItemsRemain: false,
    };
    const merged = sync.threadWindow(held, {
      items: [{ id: "m-8", data: { sequence: 8 } }],
      has_more: true,
    });
    expect(merged.items.map((one) => one.id)).toEqual(["m-7", "m-8"]);
    expect(merged.deliveredSequence).toBe(8);
    expect(merged.olderItemsRemain).toBe(false);
  });

  it("does not lose a provisional send or a newer push when a gapped page lands", () => {
    const held = {
      items: [
        { id: "m-7", data: { sequence: 7 } },
        { id: "m-158", data: { sequence: 158 } },
        { id: "pending", data: { provisional: true, operation_id: "op-1", body: "still sending" } },
      ],
      deliveredSequence: 158,
      knownTotalItems: 158,
    };
    const merged = sync.threadWindow(held, {
      items: [{ id: "m-58", data: { sequence: 58 } }, { id: "m-157", data: { sequence: 157 } }],
      has_more: true,
      thread_last_sequence: 157,
      thread_total: 157,
    }, { newest: true });
    expect(merged.items.map((one) => one.id)).toEqual(["m-58", "m-157", "m-158", "pending"]);
    expect(merged.deliveredSequence).toBe(158);
    expect(merged.knownTotalItems).toBe(158);
    expect(merged.olderItemsRemain).toBe(true);
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

  // A record made by a send is not a window: the reader wrote one message into
  // a conversation this cache had never read, so it says nothing about how far
  // back the conversation goes. The page that arrives IS that window — take
  // its `has_more`, or "load earlier" is off for the life of the record and
  // the reader can never scroll past the hundred items the page carried.
  it("lets the first page say how far back a record a send created reaches", async () => {
    const conversations = await import("../src/core/conversationCache.js");
    await conversations.writeProvisionalMessage(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      "op-1",
      { body: "ship it" },
    );
    script["thread.page"] = () => ({ items: [{ id: "m-1", data: { sequence: 500 } }], has_more: true, thread_total: 600 });

    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);

    expect(paramsOf("thread.page")).toEqual([
      { entity_id: "run-1", agent_id: "ag-1", limit: sync.LATEST_THREAD_ITEMS },
    ]);
    const record = await read("run-1", "thread", "ag-1");
    expect(record.value.olderItemsRemain).toBe(true);
    expect(record.value.knownTotalItems).toBe(600);
    expect(record.value.deliveredSequence).toBe(500);
    // And the reader's own message is still standing in it.
    expect(record.value.items.map((one) => one.data.operation_id)).toEqual([undefined, "op-1"]);
  });

  it("stores a conversation under its conversation id where it has one", async () => {
    script["thread.page"] = () => ({ items: [{ id: "m-1", data: { sequence: 1 } }], has_more: false });
    await boot([branchItem({ agents: [{ id: "ag-1", conversation_id: "conv-9" }] })]);
    expect(await read("run-1", "thread", "conv-9")).toBeTruthy();
    expect(await read("run-1", "thread", "ag-1")).toBeUndefined();
  });

  // A project's conversation is not work that finishes: there is no branch
  // behind it to merge and no row to clear, and the project page offers it
  // whenever the reader opens the project. So it is read by every pass
  // however long it has been quiet — the inbox's Recent partition is about
  // work items, and a pass that applied it here would leave a reader who
  // opens their project page a blank panel until somebody says something.
  it("reads a project's conversation on a first sync, however long it has been quiet", async () => {
    script["project.list"] = () => ({ projects: [projectList] });
    await boot([branchItem({ agents: [{ id: "ag-1" }] }), projectRow()]);
    expect(paramsOf("thread.page")).toContainEqual({
      entity_id: "run-proj",
      agent_id: "project-01M2",
      limit: 100,
    });
  });

  // The project page is a conversation page like a workspace's, so it gets
  // what one gets: the pass leads with it, and the realtime subscription
  // follows the reader onto it. Without this the page names no entity at all —
  // the one page in the app that heard nothing while its agent worked.
  it("follows the project's conversation while the reader is standing on the project page", async () => {
    script["project.list"] = () => ({ projects: [projectList] });
    await boot([branchItem({ agents: [{ id: "ag-1" }] }), projectRow()], {
      name: "project",
      deviceId: "dev-1",
      projectId: "p1",
    });

    const active = registeredWatchers.find((watcher) => watcher.id === "s-active");
    expect(active).toBeDefined();
    expect(active.entity).toBe("run-proj");
    expect(active.mode).toBe("realtime");
    // And it is read first, ahead of the foreground, like any open workspace.
    const ahead = bridge.call.mock.calls
      .filter(([, , envelope]) => envelope?.priority !== "background")
      .map(([, params]) => params?.run_id || params?.entity_id);
    expect([...new Set(ahead)]).toEqual(["run-proj"]);
  });

  it("reads a quiet project conversation again on the pass a reconnect brings", async () => {
    script["project.list"] = () => ({ projects: [projectList] });
    script["thread.page"] = () => ({ items: [{ id: "m-1", data: { sequence: 4 } }], has_more: false });
    await boot([projectRow()]);
    contexts.get("dev-1").session = { device: "dev-1", again: true };
    stateListeners.forEach((fn) => fn());
    await settle();
    // The second read is the forward delta off what the first one stored, so
    // a resync costs a cursor rather than the conversation again.
    expect(paramsOf("thread.page").filter((params) => params.entity_id === "run-proj")).toEqual([
      { entity_id: "run-proj", agent_id: "project-01M2", limit: 100 },
      { entity_id: "run-proj", agent_id: "project-01M2", after_sequence: 4, newest: true, limit: 100 },
    ]);
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
  // `git.unpushed` answers one aggregate diff and no commit list. Which
  // commits are unpushed is the log's to say: the bridge marks every commit
  // ahead of the base on the row it writes for it.
  const ahead = (hashes) => ({
    commits: hashes.map((hash) => ({ hash, ahead_of_base: true })),
    newest: hashes[0] || null,
    reset: false,
  });

  it("reads the patch behind each unpushed commit it does not hold, under the cap", async () => {
    script["git.log"] = () => ahead(["h1", "h2"]);
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "h1" }, { hash: "h1" });
    await boot([branchItem()]);
    expect(paramsOf("git.show")).toEqual([
      { run_id: "run-1", hash: "h2", max_bytes: sync.COMMIT_PATCH_MAX_BYTES },
    ]);
    expect(sync.COMMIT_PATCH_MAX_BYTES).toBe(262144);
    expect((await read("run-1", "patch", "h2")).value.patch).toBe("diff --git");
  });

  it("keeps the file headers a commit over the cap is answered with", async () => {
    script["git.log"] = () => ahead(["h1"]);
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "h1" },
      { hash: "h1", patch: "diff --git a/one b/one\n", truncated: true, patch_bytes: 400000 },
    );
    await boot([branchItem()]);
    // Which files moved is what the cap answers with, and holding it is what
    // keeps the next pass off the wire. How they moved is the reader's ask,
    // made uncapped from the pane when they open the commit.
    expect(paramsOf("git.show")).toEqual([]);
  });

  it("reads the patches of at most twenty commits", async () => {
    script["git.log"] = () => ahead(Array.from({ length: 30 }, (_unused, index) => `h${index}`));
    await boot([branchItem()]);
    expect(calls("git.show")).toHaveLength(sync.UNPUSHED_COMMITS_MAX);
  });

  it("refuses a patch over the cap, whatever the bridge answered", async () => {
    script["git.log"] = () => ahead(["h1"]);
    script["git.show"] = () => ({ hash: "h1", patch: "x".repeat(sync.COMMIT_PATCH_MAX_BYTES + 1) });
    await boot([branchItem()]);
    expect(await read("run-1", "patch", "h1")).toBeUndefined();
  });

  it("lets go of the patch of a commit that is no longer unpushed", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "patch", sub: "gone" }, { hash: "gone" });
    await boot([branchItem()]);
    expect(await read("run-1", "patch", "gone")).toBeUndefined();
  });

  // The hunks are the largest thing a workspace holds and nobody is looking
  // at them during a pass: the record gets the stat, the files and the key,
  // and the Changes surface reads the body when it opens over it.
  it("reads the working-tree diff without its hunks, and only where the cache holds none", async () => {
    script["run.diff"] = (params) => (params.patch === false
      ? { diff_key: "d1", stat: { files_changed: 1 }, files: [{ path: "src/a.js" }] }
      : { patch: "the whole diff", diff_key: "d1" });
    await boot([branchItem()]);
    expect(paramsOf("run.diff")).toEqual([{ run_id: "run-1", patch: false }]);
    const record = (await read("run-1", "diff")).value;
    expect(record.patch, "no body was asked for or written").toBeUndefined();
    expect(record.diff_key, "and the key that names one is held").toBe("d1");
    expect(record.files).toHaveLength(1);

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

  // Issues left the board, so no board will ever name one and no pass will
  // ever fill one: the issue surface's records are written by the surface
  // alone (core/issueCache.js) and are the frame it mounts from. A pass that
  // took them would leave that frame for ever unpainted.
  it("leaves an issue's own records where the board does not name the issue", async () => {
    const issueRecord = (sub, value) =>
      cache.writeCached({ deviceId: "dev-1", entityId: "issue-7", kind: "issue", sub }, value);
    await issueRecord("get", { issue_id: "issue-7" });
    await issueRecord("stages", { stages: [] });
    await issueRecord("stage:1", { doc: "# one" });
    await cache.writeCached({ deviceId: "dev-1", entityId: "issue-7", kind: "status" }, {});

    await boot([branchItem()]);

    expect((await read("issue-7", "issue", "get")).value).toEqual({ issue_id: "issue-7" });
    expect(await read("issue-7", "issue", "stages")).toBeTruthy();
    expect(await read("issue-7", "issue", "stage:1")).toBeTruthy();
    // Everything else that entity held is still the board's to take away.
    expect(await read("issue-7", "status")).toBeUndefined();
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
    // Subscribed before the lists were asked for (#142), so what changes from
    // here on is heard while the session waits to be read again.
    expect(registeredWatchers.map((watcher) => watcher.id)).toEqual(["s-inbox", "s-background"]);

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

  // A phone freezes a backgrounded tab where it stands. A frozen tab that
  // holds this lock is doing none of the work and will never hand it back, so
  // queueing behind it for ever is a tab that reads nothing all session.
  it("does the job itself when the tab holding the lock never hands it back", async () => {
    vi.stubGlobal("navigator", { locks: { request: vi.fn(async () => undefined) } }); // never granted
    vi.resetModules();
    cache = await import("../src/core/localCache.js");
    sync = await import("../src/core/cacheSync.js");
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      board = [branchItem()];
      sync.startCacheSync();
      await settle();
      expect(bridge.call).not.toHaveBeenCalled(); // still queueing

      await vi.advanceTimersByTimeAsync(sync.LOCK_WAIT_MS);
      await settle();
      expect(calls("board.list")).toHaveLength(1);
      expect(registeredWatchers.map((watcher) => watcher.id)).toEqual(["s-inbox", "s-background"]);
    } finally {
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });
});

// One machine, two sessions: the pass out belongs to the session that started
// it, and a reconnect is a different machine's answer — possibly a different
// bridge. The reader's whole app is behind this, because the subscriptions are
// the last step of a pass: a device whose pass never finishes hears nothing
// pushed and reads nothing until something else asks.
describe("a pass that is still out when the next session lands", () => {
  /** A pass that has reached the board and is waiting on a bridge that has
   *  stopped answering — a radio that went away mid-read. */
  const stallTheBoard = () => {
    let release;
    script["board.list"] = () => new Promise((resolve) => { release = () => resolve({ items: board }); });
    return () => release?.();
  };

  it("runs for the new session once the stalled one is done, rather than dropping it", async () => {
    const release = stallTheBoard();
    board = [branchItem()];
    sync.startCacheSync();
    await settle();
    expect(calls("board.list")).toHaveLength(1);

    // The reconnect lands while the first pass is still waiting on the wire.
    contexts.get("dev-1").session = { device: "dev-1", again: true };
    stateListeners.forEach((fn) => fn());
    await settle();

    script = {};
    release();
    await settle();

    // The new session got its own pass — and with it the subscriptions, which
    // are what the reader hears everything through.
    expect(calls("board.list")).toHaveLength(2);
    expect(registeredWatchers.map((watcher) => watcher.id)).toEqual(["s-inbox", "s-background"]);
    expect(paramsOf("thread.page")).toHaveLength(0); // the agentless row has no conversation
  });

  it("stands the stalled pass down rather than letting it write for a session that has gone", async () => {
    const release = stallTheBoard();
    board = [branchItem({ agents: [{ id: "ag-1" }] })];
    sync.startCacheSync();
    await settle();

    contexts.get("dev-1").session = { device: "dev-1", again: true };
    stateListeners.forEach((fn) => fn());
    await settle();

    script = {};
    release();
    await settle();

    // The superseded pass answered its board and stopped there: every read
    // past the lists belongs to the session that is actually on the wire.
    expect(calls("board.list")).toHaveLength(2);
    expect(calls("git.status")).toHaveLength(1);
  });

  it("is one pass, not two, when the same session asks again", async () => {
    const release = stallTheBoard();
    board = [branchItem()];
    sync.startCacheSync();
    await settle();

    document.dispatchEvent(new Event("visibilitychange"));
    await settle();
    expect(calls("board.list")).toHaveLength(1);

    release();
    await settle();
  });

  it("starts a restored session's baseline when its old board read never answers", async () => {
    const before = branchItem({ agents: [{ id: "ag-before" }] });
    let release;
    script["board.list"] = () => new Promise((resolve) => { release = () => resolve({ items: [before] }); });
    board = [before];
    sync.startCacheSync();
    await settle();
    expect(calls("board.list")).toHaveLength(1);

    // The restored greeting belongs to the same session. The old bridge's
    // board call is still out and may never answer at all.
    const session = contexts.get("dev-1").session;
    board = [branchItem({ agents: [{ id: "ag-after" }] })];
    script = {};
    const refreshed = sync.syncRestoredDevice("dev-1", session);
    await refreshed;
    await settle();

    expect(calls("board.list")).toHaveLength(2);
    expect((await read("run-1", "row"))?.value?.agents).toEqual([{ id: "ag-after" }]);
    release();
    await settle();
    expect((await read("run-1", "row"))?.value?.agents).toEqual([{ id: "ag-after" }]);
  });

  it("does not write an old agent surface after the restored pass observed a newer one", async () => {
    const agent = (generation, goal) => ({
      id: "ag-1", surface_session_generation: generation, surfaces: { goal },
    });
    board = [branchItem({ agents: [agent("gen-before", "before")] })];
    const originalRead = cache.readCached;
    let release;
    const surfaceRead = vi.spyOn(cache, "readCached").mockImplementation((address) => {
      if (address.kind === "surfaces" && address.entityId === "run-1") {
        surfaceRead.mockRestore();
        return new Promise((resolve) => { release = () => resolve(undefined); });
      }
      return originalRead(address);
    });
    sync.startCacheSync();
    await settle();
    expect(release).toBeTypeOf("function");

    board = [branchItem({ agents: [agent("gen-after", "after")] })];
    await sync.syncRestoredDevice("dev-1", contexts.get("dev-1").session);
    await settle();
    expect((await read("run-1", "surfaces", "ag-1"))?.value?.surfaces?.goal).toBe("after");

    release();
    await settle();
    expect((await read("run-1", "surfaces", "ag-1"))?.value?.surfaces?.goal).toBe("after");
  });

  it("does not fold a restored issue list behind the old path's unanswered read", async () => {
    let release;
    script["issues.list"] = () => new Promise((resolve) => { release = () => resolve({ issues: [{ id: "old" }] }); });
    board = [branchItem()];
    sync.startCacheSync();
    await settle();
    expect(calls("issues.list")).toHaveLength(1);

    script = {};
    await sync.syncRestoredDevice("dev-1", contexts.get("dev-1").session);
    await settle();
    expect(calls("issues.list")).toHaveLength(2);
    expect(registeredWatchers.map((watcher) => watcher.id)).toContain("s-inbox");
    expect((await read("p1", "tracker-issues"))?.value?.issues).toEqual([]);

    release();
    await settle();
    expect(calls("issues.list")).toHaveLength(2);
    expect((await read("p1", "tracker-issues"))?.value?.issues).toEqual([]);
  });

  it("does not share a restored diff read with the old path's unanswered request", async () => {
    let release;
    script["run.diff"] = () => new Promise((resolve) => { release = () => resolve({ diff_key: "old", patch: "old" }); });
    board = [branchItem()];
    sync.startCacheSync();
    await settle();
    expect(calls("run.diff")).toHaveLength(1);

    script = {};
    await sync.syncRestoredDevice("dev-1", contexts.get("dev-1").session);
    await settle();
    expect(calls("run.diff")).toHaveLength(2);
    expect((await read("run-1", "diff"))?.value?.diff_key).toBe("d1");

    release();
    await settle();
    expect((await read("run-1", "diff"))?.value?.diff_key).toBe("d1");
  });

  it("ignores a late diff read started by a push on the old path", async () => {
    await boot([branchItem()], { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" });
    let release;
    script["run.diff"] = () => new Promise((resolve) => { release = () => resolve({ diff_key: "old", patch: "old" }); });
    const active = registeredWatchers.find((watcher) => watcher.id === "s-active" && !watcher.disposed);
    active.onChanges([{ entity_id: "run-1", git: { diff: null } }]);
    await settle();
    expect(calls("run.diff")).toHaveLength(2); // boot and the old push

    await cache.deleteCached([{ deviceId: "dev-1", entityId: "run-1", kind: "diff" }]);
    script = {};
    await sync.syncRestoredDevice("dev-1", contexts.get("dev-1").session);
    await settle();
    expect((await read("run-1", "diff"))?.value?.diff_key).toBe("d1");

    release();
    await settle();
    expect((await read("run-1", "diff"))?.value?.diff_key).toBe("d1");
  });

  it("ignores an old status push paused before its row read when the restored baseline lands", async () => {
    await boot([branchItem()], { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" });
    const heldRow = await read("run-1", "row");
    const originalRead = cache.readCached;
    let release;
    const rowRead = vi.spyOn(cache, "readCached").mockImplementation((address) => {
      if (address.entityId === "run-1" && address.kind === "row") {
        rowRead.mockRestore();
        return new Promise((resolve) => { release = () => resolve(heldRow); });
      }
      return originalRead(address);
    });
    const active = registeredWatchers.find((watcher) => watcher.id === "s-active" && !watcher.disposed);
    active.onChanges([{ entity_id: "run-1", git: { status: { head: "old-push" } } }]);
    await settle();
    expect(release).toBeTypeOf("function");

    script["git.status"] = () => ({ head: "restored-baseline", status_key: "restored" });
    await sync.syncRestoredDevice("dev-1", contexts.get("dev-1").session);
    await settle();
    expect((await read("run-1", "status"))?.value?.head).toBe("restored-baseline");

    release();
    await settle();
    expect((await read("run-1", "status"))?.value?.head).toBe("restored-baseline");
  });

  // The greeting settles when the bridge answers, when the session dies, or
  // when a newer session arms its own. A session whose transport is up and
  // whose bridge says nothing settles none of those.
  it("stands down when the greeting never settles, leaving the device askable again", async () => {
    contexts.get("dev-1").greeted = new Promise(() => {});
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      board = [branchItem()];
      sync.startCacheSync();
      await settle();
      expect(bridge.call).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(sync.GREETING_WAIT_MS);
      await settle();
      expect(bridge.call).not.toHaveBeenCalled(); // it gave up rather than asking blind

      // And the device is not poisoned: the greeting landing is an
      // announcement, and the pass behind it reads the machine.
      contexts.get("dev-1").greeted = Promise.resolve();
      stateListeners.forEach((fn) => fn());
      await settle();
      expect(calls("board.list")).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
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

// Every project's issue list, read on every pass for the same reason its own
// conversation is: the Issues tab offers itself the moment the reader opens a
// project, and a project holds no work that finishes, so it never ages into
// Recent to be rescued by being routed to.
describe("every project's issues", () => {
  const issuesOf = (projectId) => read(projectId, "tracker-issues");

  it("reads the list of every project the device lists", async () => {
    script["project.list"] = () => ({ projects: [{ project_id: "p1" }, { project_id: "p2" }] });
    await boot([]);
    expect(paramsOf("issues.list")).toEqual([{ project_id: "p1" }, { project_id: "p2" }]);
  });

  // Not only when routed, and with no work item in the project at all.
  it("reads a project nobody has cut a workspace in", async () => {
    script["issues.list"] = () => ({ issues: [{ id: "issue-1", number: 12, status: "backlog" }] });
    await boot([]);
    expect((await issuesOf("p1")).value.issues.map((one) => one.number)).toEqual([12]);
  });

  // The tab's filters are `issues.list` params of their own; a record already
  // narrowed would be missing whatever the next filter is about to ask for.
  it("asks for the whole list, narrowed by nothing", async () => {
    await boot([]);
    expect(paramsOf("issues.list")).toEqual([{ project_id: "p1" }]);
  });

  // #129: the Issues tab weighs this list against its own filtered answer by
  // when each was asked, and this read is large enough to land well after.
  it("stamps the list with when it was asked, not when it landed", async () => {
    script["issues.list"] = () => new Promise((resolve) => setTimeout(() => resolve({ issues: [] }), 50));
    await boot([]);
    await vi.waitFor(async () => expect(await issuesOf("p1")).toBeTruthy());
    const landed = await issuesOf("p1");
    expect(landed.value.read_order).toBeLessThan(landed.at - 25);
  });

  it("holds the project's columns beside its issues", async () => {
    await boot([]);
    expect((await issuesOf("p1")).value.columns).toEqual([{ id: "backlog", name: "Backlog" }]);
  });

  // The columns change with the project, not with an issue.
  it("asks for the columns once and not again on the next pass", async () => {
    await boot([]);
    await sync.syncDevice("dev-1");
    await settle();
    expect(calls("issues.columns")).toHaveLength(1);
    expect(calls("issues.list")).toHaveLength(2);
  });

  // An issue list is a project surface; the inbox is the landing one.
  it("reads them behind the workspaces, never in front", async () => {
    await boot([branchItem()]);
    const order = bridge.call.mock.calls.map(([method]) => method);
    expect(order.indexOf("issues.list")).toBeGreaterThan(order.indexOf("git.status"));
  });

  // A project holds records but is not a board row: the pass that drops what
  // the board stopped naming must not take them.
  it("keeps a listed project's issues through the drop pass", async () => {
    await boot([branchItem()]);
    expect(await issuesOf("p1")).toBeTruthy();
    board = [];
    await sync.syncDevice("dev-1");
    await settle();
    expect(await issuesOf("p1")).toBeTruthy();
  });

  it("drops the issues of a project the device has stopped listing", async () => {
    await boot([]);
    expect(await issuesOf("p1")).toBeTruthy();
    script["project.list"] = () => ({ projects: [{ project_id: "p2" }] });
    await sync.syncDevice("dev-1");
    await settle();
    expect(await issuesOf("p1")).toBeUndefined();
  });

  // #119: a busy project pushes every flush. Each push used to start its own
  // read of the whole list, all of them crossing the wire at once; a push
  // heard while that read is out now waits for it and reads once after it.
  it("reads a project's list once more after a burst of pushes, never alongside", async () => {
    await boot([]);
    const before = calls("issues.list").length;
    const answers = [];
    script["issues.list"] = () => new Promise((resolve) => answers.push(resolve));
    const inbox = registeredWatchers.find((watcher) => watcher.id === "s-inbox");
    const pushed = { entity_id: "p1", issues: { issue_ids: ["issue-1"], truncated: false } };
    for (let n = 0; n < 4; n += 1) inbox.onChanges([pushed]);
    await vi.waitFor(() => expect(answers).toHaveLength(1));
    expect(calls("issues.list")).toHaveLength(before + 1);

    answers[0]({ issues: [{ id: "issue-1", number: 12, status: "done" }] });
    await vi.waitFor(() => expect(answers).toHaveLength(2));
    expect(calls("issues.list")).toHaveLength(before + 2);
    answers[1]({ issues: [{ id: "issue-1", number: 12, status: "done" }] });
    await vi.waitFor(async () =>
      expect((await issuesOf("p1")).value.issues.map((one) => one.status)).toEqual(["done"]));
  });

  // A bridge that predates the tracker refuses both verbs; the tab falls back
  // to phase 1's five columns and nothing reaches the reader.
  it("writes nothing when the bridge does not serve the tracker", async () => {
    script["issues.list"] = () => {
      throw new Error("unknown method");
    };
    await boot([]);
    expect(await issuesOf("p1")).toBeUndefined();
  });
});
