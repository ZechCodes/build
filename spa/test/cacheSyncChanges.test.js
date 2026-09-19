// @vitest-environment jsdom
// The three subscriptions, and what one pushed item writes.
//
// A push carries bodies, so applying one is writes: the item says what moved
// and the record is replaced with what it says. The only things pulled in
// response are the two that never ride a push — the patch behind a commit and
// a working-tree diff too big to send — and the deeper directory listings the
// bridge cannot know the reader walked into.

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

let cache, sync;
let board = [];
let script = {};

const ANSWERS = {
  "board.list": () => ({ items: board }),
  "project.list": () => ({ projects: [{ project_id: "p1", name: "build" }] }),
  "workspace.list": () => ({ workspaces: [] }),
  "git.status": () => ({ head: "abc", status_key: "key-1", files: [] }),
  "git.log": () => ({ commits: [{ hash: "c1" }], newest: "c1", reset: false }),
  "git.unpushed": () => ({ base: {}, commits: [], diff_key: "d1" }),
  "git.show": (params) => ({ hash: params.hash, patch: "diff --git" }),
  "term.list": () => ({ terminals: [] }),
  "fs.tree": (params) => ({ path: params.path, entries: [] }),
  "thread.page": () => ({ items: [], has_more: false }),
  "run.diff": () => ({ patch: "pulled", diff_key: "d2" }),
  "worktree.diff": () => ({ patch: "pulled", diff_key: "d2" }),
};

const answer = (method, params) => (script[method] || ANSWERS[method] || (() => ({})))(params || {});

const settle = async () => {
  let before = -1;
  while (before !== bridge.call.mock.calls.length) {
    before = bridge.call.mock.calls.length;
    for (let turn = 0; turn < 12; turn += 1) await new Promise((done) => setTimeout(done, 0));
  }
};

const boot = async (items = [branchItem()], route = { name: "inbox" }) => {
  board = items;
  App.route = route;
  sync.startCacheSync();
  await settle();
};

const live = () => registeredWatchers.filter((watcher) => !watcher.disposed);
const subscription = (id) => live().find((watcher) => watcher.id === id) || null;

/** One flush, delivered to every subscription that would have carried it. */
const deliver = async (items, kinds = ["state", "thread", "git", "files", "terminals"]) => {
  for (const watcher of live()) {
    if (!watcher.kinds.some((kind) => kinds.includes(kind))) continue;
    watcher.onChanges(items);
    break; // the bridge sends one flush per subscription; one delivery is one flush
  }
  await settle();
};

const calls = (method) => bridge.call.mock.calls.filter(([name]) => name === method);
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
});

describe("the three subscriptions", () => {
  const shapeOf = (watcher) => [watcher.id, watcher.scope || watcher.entity, watcher.kinds, watcher.mode, watcher.priority];

  it("takes out exactly three per device, and no more on a second pass", async () => {
    await boot([branchItem()], { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" });
    expect(live().map(shapeOf)).toEqual([
      ["s-inbox", "all", ["state", "thread"], "realtime", "foreground"],
      ["s-background", "all", ["git", "files", "terminals"], { batch_ms: sync.BACKGROUND_COOLDOWN_MS }, "background"],
      ["s-active", "run-1", ["git", "files", "terminals"], "realtime", "foreground"],
    ]);
    expect(sync.BACKGROUND_COOLDOWN_MS).toBe(30000);

    document.dispatchEvent(new Event("visibilitychange"));
    await settle();
    expect(live()).toHaveLength(3);
  });

  it("takes out no active subscription where the route names no workspace", async () => {
    await boot([branchItem()]);
    expect(live().map((watcher) => watcher.id)).toEqual(["s-inbox", "s-background"]);
  });

  it("re-issues the active one for the workspace the reader moved to", async () => {
    const two = [branchItem(), branchItem({ branch: "build/search", run_id: "run-2", worktree_id: "wt-2" })];
    await boot(two, { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" });
    const first = subscription("s-active");
    expect(first.entity).toBe("run-1");

    App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/search" };
    sync.routeChanged();
    await settle();

    expect(first.disposed).toBe(true);
    expect(subscription("s-active").entity).toBe("run-2");
    expect(live()).toHaveLength(3);
  });

  it("issues the active one for a workspace whose row arrived on a push", async () => {
    // The commonest way onto a new workspace: the reader makes a branch and
    // walks into it. Its row rides a `state` item — the last pass never saw
    // it — and the workspace on screen still owes realtime git and files.
    await boot([branchItem()]);
    await deliver([{
      entity_id: "run-7",
      state: branchItem({ branch: "build/search", run_id: "run-7", worktree_id: "wt-7" }),
    }]);

    App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/search" };
    sync.routeChanged();
    await settle();

    expect(subscription("s-active")?.entity).toBe("run-7");
  });

  it("follows a route taken while the pass was still reading", async () => {
    // A pass reads six or eight shapes per workspace, one after another, and
    // the reader does not wait for it. Where they are standing when the
    // subscriptions are taken out is where they are standing now, not where
    // the pass found them when it started.
    const two = [branchItem(), branchItem({ branch: "build/search", run_id: "run-2", worktree_id: "wt-2" })];
    script["git.unpushed"] = () => {
      App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/search" };
      sync.routeChanged();
      return { base: {}, commits: [], diff_key: "d1" };
    };
    await boot(two, { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" });

    expect(subscription("s-active")?.entity).toBe("run-2");
    expect(live()).toHaveLength(3);
  });

  it("does not put a later pass's reader back in the workspace they left", async () => {
    const two = [branchItem(), branchItem({ branch: "build/search", run_id: "run-2", worktree_id: "wt-2" })];
    await boot(two, { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" });
    expect(subscription("s-active")?.entity).toBe("run-1");

    script["git.unpushed"] = () => {
      App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/search" };
      sync.routeChanged();
      return { base: {}, commits: [], diff_key: "d1" };
    };
    document.dispatchEvent(new Event("visibilitychange"));
    await settle();

    expect(subscription("s-active")?.entity).toBe("run-2");
    expect(live()).toHaveLength(3);
  });

  it("lets the active one go when the reader leaves the workspace", async () => {
    await boot([branchItem()], { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" });
    App.route = { name: "inbox" };
    sync.routeChanged();
    await settle();
    expect(subscription("s-active")).toBeNull();
    expect(live().map((watcher) => watcher.id)).toEqual(["s-inbox", "s-background"]);
  });

  it("hears a flush in a hidden tab — a push carries what it would have read", async () => {
    await boot([branchItem()]);
    expect(live().every((watcher) => watcher.pausesWhileHidden === false)).toBe(true);
  });

  it("takes them all down when the syncer stops", async () => {
    await boot([branchItem()], { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" });
    sync.stopCacheSync();
    expect(live()).toEqual([]);
  });
});

describe("applying one item", () => {
  it("writes the row a state item carries, and nothing else", async () => {
    await boot([branchItem()]);
    bridge.call.mockClear();
    const moved = { ...branchItem(), state: "review", unread_reason: "done" };
    await deliver([{ entity_id: "run-1", state: moved }]);
    expect((await read("run-1", "row")).value).toMatchObject({ state: "review", deviceId: "dev-1" });
    expect(bridge.call).not.toHaveBeenCalled();
  });

  // A legacy issue left the board, so the bridge has no row to push for one:
  // its `state` item is the three-field digest it always answered with, whose
  // `agents` is a COUNT. Written as if it were a row, it is a work item whose
  // agents cannot be walked, and every reader of that record is handed one.
  it("leaves the row alone for a state item that is not a row", async () => {
    await boot([branchItem()]);
    await deliver([{ entity_id: "plan-1", state: { run: "planning", agents: 2, attention: "none" } }]);
    expect(await read("plan-1", "row")).toBeUndefined();
  });

  it("lets a finished workspace's data go, and keeps its row", async () => {
    await boot([branchItem()]);
    expect(await read("run-1", "status")).toBeTruthy();
    await deliver([{ entity_id: "run-1", state: { ...branchItem(), state: "merged" } }]);
    expect(await read("run-1", "status")).toBeUndefined();
    expect(await read("run-1", "log")).toBeUndefined();
    expect((await read("run-1", "row")).value.state).toBe("merged");
  });

  it("writes the git shapes a git item carries, and asks for none of them", async () => {
    await boot([branchItem()]);
    bridge.call.mockClear();
    await deliver([{
      entity_id: "run-1",
      git: {
        status: { head: "def", status_key: "key-2", files: [] },
        log: { commits: [{ hash: "c2" }, { hash: "c1" }], newest: "c2", more: false },
        unpushed: { base: {}, commits: [], diff_key: "d9", patch: "megabytes" },
        diff: { patch: "pushed body", diff_key: "d9" },
      },
    }]);
    expect((await read("run-1", "status")).value.status_key).toBe("key-2");
    expect((await read("run-1", "log")).value.commits.map((one) => one.hash)).toEqual(["c2", "c1"]);
    expect((await read("run-1", "unpushed")).value.patch).toBeUndefined();
    expect((await read("run-1", "diff")).value.patch).toBe("pushed body");
    expect(calls("git.status")).toEqual([]);
    expect(calls("git.log")).toEqual([]);
    expect(calls("run.diff")).toEqual([]);
  });

  it("replaces the commit record when a pushed history no longer reaches the hash it held", async () => {
    // A `git` item's commits are the head of the history as it stands, never a
    // delta: a push has no cursor to answer from. So a window that does not
    // reach back to the hash the record was reading forward from is a history
    // that moved under this cache — a rebase, a reset — and the commits it
    // held are ones this checkout no longer has.
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "log" },
      { commits: [{ hash: "c3" }, { hash: "c2" }, { hash: "c1" }], newest: "c3" },
    );
    // The pass reads forward from c3 and hears of nothing new.
    script["git.log"] = () => ({ commits: [], newest: "c3" });
    await boot([branchItem()]);

    await deliver([{
      entity_id: "run-1",
      git: { log: { commits: [{ hash: "d3" }, { hash: "d2" }, { hash: "c1" }], newest: "d3", more: false } },
    }]);

    const record = (await read("run-1", "log")).value;
    expect(record.commits.map((one) => one.hash)).toEqual(["d3", "d2", "c1"]);
    expect(record.newest).toBe("d3");
  });

  it("keeps the commits behind a pushed history that still reaches them", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "log" },
      { commits: [{ hash: "c2" }, { hash: "c1" }], newest: "c2" },
    );
    script["git.log"] = () => ({ commits: [], newest: "c2" });
    await boot([branchItem()]);

    await deliver([{
      entity_id: "run-1",
      git: { log: { commits: [{ hash: "c4" }, { hash: "c3" }, { hash: "c2" }], newest: "c4", more: true } },
    }]);

    const record = (await read("run-1", "log")).value;
    expect(record.commits.map((one) => one.hash)).toEqual(["c4", "c3", "c2", "c1"]);
    expect(record.newest).toBe("c4");
  });

  it("keeps the project and the triage beside a diff a git item replaces", async () => {
    await boot([branchItem()]);
    const held = (await read("run-1", "diff")).value;
    expect(held.projectId).toBe("p1");
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "diff", sub: "" },
      { ...held, triage: { "src/a.js": "reviewed" } },
    );

    await deliver([{ entity_id: "run-1", git: { diff: { patch: "pushed body", diff_key: "d9" } } }]);
    const after = (await read("run-1", "diff")).value;
    expect(after.patch).toBe("pushed body");
    expect(after.diff_key).toBe("d9");
    expect(after.projectId).toBe("p1");
    expect(after.triage).toEqual({ "src/a.js": "reviewed" });
  });

  it("reads the patch behind a commit the git item made unpushed", async () => {
    await boot([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", git: { log: { commits: [{ hash: "h7", ahead_of_base: true }], newest: "h7" } } }]);
    expect(calls("git.show").map(([, params]) => params.hash)).toEqual(["h7"]);
    expect((await read("run-1", "patch", "h7")).value.patch).toBe("diff --git");
  });

  it("pulls a diff the bridge could not send, for the workspace on screen only", async () => {
    await boot([branchItem()], { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" });
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", git: { diff: null, diff_bytes: 900000 } }]);
    expect(calls("run.diff")).toHaveLength(1);

    App.route = { name: "inbox" };
    sync.routeChanged();
    await settle();
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", git: { diff: null, diff_bytes: 900000 } }]);
    expect(calls("run.diff")).toEqual([]);
  });

  // A surface snapshot is process-local: it rides the agent digest while the
  // session that observed it is alive, and the record is what a reader who
  // comes back to a dead one is shown. Only this layer writes it.
  it("writes the surface snapshot a pushed row's agent carries", async () => {
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    await deliver([{
      entity_id: "run-1",
      state: branchItem({
        agents: [{
          id: "ag-1",
          surface_session_generation: "gen-1",
          surfaces: { goal: { text: "land the rail" } },
        }],
      }),
    }]);

    const record = await read("run-1", "surfaces", "ag-1");
    expect(record.value).toEqual({ surfaces: { goal: { text: "land the rail" } }, generation: "gen-1" });
  });

  it("leaves the surface record alone while the snapshot has not moved", async () => {
    const agents = [{ id: "ag-1", surface_session_generation: "gen-1", surfaces: { goal: { text: "land the rail" } } }];
    await boot([branchItem({ agents })]);
    await deliver([{ entity_id: "run-1", state: branchItem({ agents }) }]);
    const first = await read("run-1", "surfaces", "ag-1");
    await new Promise((done) => setTimeout(done, 2));
    await deliver([{ entity_id: "run-1", state: branchItem({ agents }) }]);

    expect((await read("run-1", "surfaces", "ag-1")).at).toBe(first.at);
  });

  // A session that has stopped observing anything says so by carrying nothing,
  // and the record has to hear it: a reader coming back to that agent would
  // otherwise be painted the last snapshot as though it still stood.
  it("clears the record when a live session observes nothing at all", async () => {
    const observing = [{ id: "ag-1", surface_session_generation: "gen-1", surfaces: { goal: { text: "land it" } } }];
    await boot([branchItem({ agents: observing })]);
    await deliver([{ entity_id: "run-1", state: branchItem({ agents: observing }) }]);
    expect((await read("run-1", "surfaces", "ag-1")).value.surfaces).toEqual({ goal: { text: "land it" } });

    await deliver([{
      entity_id: "run-1",
      state: branchItem({ agents: [{ id: "ag-1", surface_session_generation: "gen-1" }] }),
    }]);

    expect((await read("run-1", "surfaces", "ag-1")).value).toEqual({ surfaces: null, generation: "gen-1" });
  });

  it("keeps no surface record for an agent with no session to have observed one", async () => {
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    await deliver([{ entity_id: "run-1", state: branchItem({ agents: [{ id: "ag-1" }] }) }]);
    expect(await read("run-1", "surfaces", "ag-1")).toBeUndefined();
  });

  it("appends the items a thread item carries and moves the cursor", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 7 } }], deliveredSequence: 7 },
    );
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    bridge.call.mockClear();
    await deliver([{
      entity_id: "run-1",
      thread: [{ agent_id: "ag-1", last_sequence: 9, since_sequence: 7, items: [{ id: "m-2", data: { sequence: 9 } }] }],
    }]);
    const record = await read("run-1", "thread", "ag-1");
    expect(record.value.items.map((one) => one.id)).toEqual(["m-1", "m-2"]);
    expect(record.value.deliveredSequence).toBe(9);
    expect(calls("thread.page")).toEqual([]);
  });

  // A message sent from this tab stands in the record until the conversation
  // carries it. The push that carries it is what takes the stand-in away —
  // nobody should see their own message twice, once queued and once sent.
  it("takes this tab's stand-in out of the record when the real item arrives", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      {
        items: [
          { id: "m-1", data: { sequence: 7 } },
          { data: { provisional: true, operation_id: "op-1", sequence: 9, role: "user", body: "ship it" } },
        ],
        deliveredSequence: 7,
      },
    );
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    bridge.call.mockClear();
    await deliver([{
      entity_id: "run-1",
      thread: [{
        agent_id: "ag-1",
        last_sequence: 9,
        since_sequence: 7,
        items: [{ id: "m-2", data: { sequence: 9, role: "user", body: "ship it" } }],
      }],
    }]);
    const record = await read("run-1", "thread", "ag-1");
    expect(record.value.items.map((one) => one.data.sequence)).toEqual([7, 9]);
    expect(record.value.items.map((one) => one.id)).toEqual(["m-1", "m-2"]);
  });

  // The first word on a conversation makes the record: there was nothing on
  // disk to merge into, so the record IS the stand-in until the wire says
  // otherwise. The item that arrives names the operation that made it, which
  // is the only thing that can say so — the send never heard a sequence.
  it("takes a stand-in out of a record the send itself created", async () => {
    const conversations = await import("../src/core/conversationCache.js");
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" };
    await conversations.writeProvisionalMessage(address, "op-1", { body: "ship it" });
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    bridge.call.mockClear();

    await deliver([{
      entity_id: "run-1",
      thread: [{
        agent_id: "ag-1",
        last_sequence: 1,
        since_sequence: 0,
        items: [{ data: { sequence: 1, role: "user", body: "ship it", operation_id: "op-1" } }],
      }],
    }]);

    const record = await read("run-1", "thread", "ag-1");
    expect(record.value.items.map((one) => one.data.sequence)).toEqual([1]);
    expect(record.value.items.map((one) => one.data.provisional)).toEqual([undefined]);
  });

  it("reads one cursored page for a tip that outran the push cap", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 7 } }], deliveredSequence: 7 },
    );
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    bridge.call.mockClear();
    script["thread.page"] = () => ({ items: [{ id: "m-9", data: { sequence: 400 } }], has_more: false });
    await deliver([{ entity_id: "run-1", thread: [{ agent_id: "ag-1", last_sequence: 400, items: [] }] }]);
    expect(calls("thread.page").map(([, params]) => params)).toEqual([
      { entity_id: "run-1", agent_id: "ag-1", after_sequence: 7, limit: sync.LATEST_THREAD_ITEMS },
    ]);
    expect((await read("run-1", "thread", "ag-1")).value.deliveredSequence).toBe(400);
  });

  it("pages forward rather than append over a gap the tip left", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 7 } }], deliveredSequence: 7 },
    );
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    bridge.call.mockClear();
    // The flush before this one was answered by a `thread.page` that failed:
    // the bridge's cursor moved to 25, the record's did not.
    script["thread.page"] = () => ({ items: [{ id: "m-8", data: { sequence: 8 } }], has_more: false });
    await deliver([{
      entity_id: "run-1",
      thread: [{ agent_id: "ag-1", last_sequence: 30, since_sequence: 25, items: [{ id: "m-30", data: { sequence: 30 } }] }],
    }]);
    expect(calls("thread.page").map(([, params]) => params.after_sequence)).toEqual([7]);
    const record = await read("run-1", "thread", "ag-1");
    expect(record.value.items.map((one) => one.id)).toEqual(["m-1", "m-8"]);
  });

  it("asks for nothing on a tip the record has already heard", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 7 } }], deliveredSequence: 7 },
    );
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", thread: [{ agent_id: "ag-1", last_sequence: 7 }] }]);
    expect(bridge.call).not.toHaveBeenCalled();
  });

  it("writes the root listing a files item carries and re-lists only what a changed path stales", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, { path: "src", entries: [] });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "docs" }, { path: "docs", entries: [] });
    await boot([branchItem()]);
    bridge.call.mockClear();
    await deliver([{
      entity_id: "run-1",
      files: { paths: ["src/a.js"], truncated: false, root: { path: "", entries: [{ name: "src", kind: "dir" }] } },
    }]);
    expect(calls("fs.tree").map(([, params]) => params.path)).toEqual(["src"]);
    expect((await read("run-1", "tree", "")).value.entries).toEqual([{ name: "src", kind: "dir" }]);
  });

  it("re-lists every directory the reader walked into when the changed list was truncated", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, { path: "src", entries: [] });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "docs" }, { path: "docs", entries: [] });
    await boot([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", files: { paths: [], truncated: true } }]);
    expect(calls("fs.tree").map(([, params]) => params.path).sort()).toEqual(["", "docs", "src"]);
  });

  it("writes the tab list a terminals item carries", async () => {
    await boot([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-1", terminals: { tabs: [{ term_id: "term-3", title: "shell" }] } }]);
    expect((await read("run-1", "terminals")).value.tabs).toEqual([{ term_id: "term-3", title: "shell" }]);
    expect(calls("term.list")).toEqual([]);
  });
});

describe("a push and a pass landing on the same record", () => {
  // Both are read-then-merge-then-write and neither waits for the other. A
  // record whose merge saw the store as it was before the other writer got
  // there would put that writer's arrival back the way it was.

  /** A pass held at one verb: the answer resolves when the case says so. */
  const heldAnswer = (method) => {
    let release = null;
    script[method] = () => new Promise((resolve) => {
      release = resolve;
    });
    return (answered) => release(answered);
  };

  it("keeps a commit a push carried while the pass was reading the log", async () => {
    await boot([branchItem()]);
    const answerLog = heldAnswer("git.log");
    document.dispatchEvent(new Event("visibilitychange"));
    await settle();

    await deliver([{
      entity_id: "run-1",
      git: { log: { commits: [{ hash: "c2" }, { hash: "c1" }], newest: "c2", more: false } },
    }]);
    answerLog({ commits: [], newest: "c1" });
    await settle();

    const record = (await read("run-1", "log")).value;
    expect(record.commits.map((one) => one.hash)).toEqual(["c2", "c1"]);
    expect(record.newest).toBe("c2");
  });

  it("keeps the items a push carried while the pass was reading the conversation", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" },
      { items: [{ id: "m-1", data: { sequence: 7 } }], deliveredSequence: 7 },
    );
    await boot([branchItem({ agents: [{ id: "ag-1" }] })]);
    const answerPage = heldAnswer("thread.page");
    document.dispatchEvent(new Event("visibilitychange"));
    await settle();

    await deliver([{
      entity_id: "run-1",
      thread: [{
        agent_id: "ag-1",
        since_sequence: 7,
        last_sequence: 9,
        items: [{ id: "m-8", data: { sequence: 8 } }, { id: "m-9", data: { sequence: 9 } }],
      }],
    }]);
    answerPage({ items: [{ id: "m-8", data: { sequence: 8 } }], has_more: false });
    await settle();

    const record = (await read("run-1", "thread", "ag-1")).value;
    expect(record.items.map((one) => one.id)).toEqual(["m-1", "m-8", "m-9"]);
    expect(record.deliveredSequence).toBe(9);
  });
});

describe("the board item", () => {
  it("lets go of the data of every entity that left the board", async () => {
    await boot([branchItem()]);
    expect(await read("run-1", "status")).toBeTruthy();
    await deliver([{ entity_id: "board", state: { revision: 4, removed: ["run-1"] } }]);
    expect(await read("run-1", "status")).toBeUndefined();
    // The row is the board's to remove; the next pass finds it unnamed.
    expect(await read("run-1", "row")).toBeTruthy();
  });

  it("writes the lists it carries, stamped with the device that sent them", async () => {
    await boot([branchItem()]);
    await deliver([{
      entity_id: "board",
      state: { revision: 5, projects: [{ project_id: "p2", name: "relaydb" }], workspaces: [{ id: "ws-2", project_id: "p2" }] },
    }]);
    expect((await read("", "projects")).value).toEqual([
      expect.objectContaining({ id: "p2", deviceId: "dev-1", projectKey: expect.any(String) }),
    ]);
    expect((await read("", "workspaces")).value[0]).toMatchObject({ id: "ws-2", deviceId: "dev-1" });
  });

  it("keeps the board's verdict on a workspace whose list it re-carries", async () => {
    // Done is the board's to decide, and `board.list` is the only read that
    // answers it: the item's workspace list carries none. Re-stamping that
    // list over the record must not take the verdict off every row.
    script["workspace.list"] = () => ({ workspaces: [{ id: "ws-2", project_id: "p1" }] });
    script["board.list"] = () => ({
      items: board,
      workspace_summaries: [
        { workspace_id: "ws-2", work_summary: "3 files changed", can_finish: true, finish_blockers: [] },
      ],
    });
    await boot([branchItem()]);

    await deliver([{
      entity_id: "board",
      state: { revision: 7, workspaces: [{ id: "ws-2", project_id: "p1" }] },
    }]);

    expect((await read("", "workspaces")).value[0]).toMatchObject({
      id: "ws-2",
      work_summary: "3 files changed",
      can_finish: true,
      finish_blockers: [],
    });
  });

  it("leaves the lists alone when the board item names neither", async () => {
    await boot([branchItem()]);
    const before = (await read("", "projects")).value;
    await deliver([{ entity_id: "board", state: { revision: 6 } }]);
    expect((await read("", "projects")).value).toEqual(before);
  });
});

describe("an item for somewhere else", () => {
  it("writes an entity's row even before a pass has read it", async () => {
    await boot([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-404", state: { ...branchItem(), run_id: "run-404" } }]);
    expect((await read("run-404", "row")).value.run_id).toBe("run-404");
  });

  it("asks nothing of git for an entity whose row this cache has never seen", async () => {
    await boot([branchItem()]);
    bridge.call.mockClear();
    await deliver([{ entity_id: "run-404", git: { log: { commits: [{ hash: "h1", ahead_of_base: true }], newest: "h1" } } }]);
    expect(calls("git.show")).toEqual([]);
  });
});
