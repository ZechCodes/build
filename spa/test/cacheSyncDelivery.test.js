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
  it("syncs and retains the conversation of an unwatched workspace run", async () => {
    const hiddenRun = {
      run_id: "run-quiet", project_id: "p1", state: "building",
      agents: [{ id: "ag-quiet", conversation_id: "conv-quiet", watched: false }],
    };
    bridge.call = vi.fn(async (method, params) => {
      if (method === "board.list") return { items: [], runs: [hiddenRun] };
      if (method === "workspace.list") return { workspaces: [{
        id: "ws-quiet", workspace_id: "ws-quiet", project_id: "p1", name: "Quiet work", entity_id: "run-quiet",
      }] };
      if (method === "thread.page") return {
        items: [{ id: "m-1", type: "message", data: { sequence: 1, role: "agent", body: "cached reply" } }],
        has_more: false, thread_total: 1, thread_last_sequence: 1,
      };
      return answer(method, params);
    });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-quiet", kind: "row" }, {
      ...hiddenRun, agents: [{ id: "ag-previously-watched", watched: true }],
    });
    await new Promise((done) => setTimeout(done, 2));

    await boot([], { name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: "ws-quiet" });

    expect(calls("thread.page").map(([, params]) => params)).toContainEqual(expect.objectContaining({
      entity_id: "run-quiet", agent_id: "ag-quiet",
    }));
    expect((await read("run-quiet", "thread", "conv-quiet")).value.items[0].data.body).toBe("cached reply");
    expect(await read("run-quiet", "row")).toBeUndefined();
  });

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

// #142: the bridge starts a subscription empty and records a change only for
// the subscriptions it holds when the change happens (bridge/src/changes.rs).
// A pass that read the board before its subscriptions were held lost whatever
// changed in between: not in the snapshot, and pushed to nobody.
describe("a pass racing the wire", () => {
  const HELLO = {
    api_version: "1.21.0",
    push_events: true,
    changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "issues"], items: "bodies" },
  };

  /** A bridge that holds subscriptions the way the real one does, and a board
   *  that moves when the test says so: `during(method, change)` runs `change`
   *  the first time `method` is asked, before it answers. */
  const subscribingBridge = () => {
    const held = new Set();
    const pending = new Map();
    const overrides = new Map();
    const gates = new Map();
    const wire = {
      held,
      during: (method, change) => pending.set(method, change),
      /** Answer `method` this way from now on; a throw is a refusal. */
      answering: (method, reply) => overrides.set(method, reply),
      /** Hold every answer to `method` (the bridge still does the work) until
       *  the returned function is called. */
      hold: (method, match = () => true) => {
        let open;
        const opened = new Promise((done) => { open = done; });
        gates.set(method, { opened, match });
        return () => {
          gates.delete(method);
          open();
        };
      },
      /** What the bridge pushes for one entity's row: to `s-inbox`, if it is held. */
      pushRow: (entityId) => {
        if (!held.has("s-inbox")) return;
        const state = structuredClone(board.find((row) => row.run_id === entityId));
        changeEvents.dispatchChangeEvent({ type: "changes", subscription_id: "s-inbox", items: [{ entity_id: entityId, state }] }, "dev-1");
      },
    };
    const reply = async (method, params) => {
      const answered = await respond(method, params);
      const gate = gates.get(method);
      if (gate?.match(params)) await gate.opened;
      return answered;
    };
    const respond = async (method, params) => {
      if (method === "session.hello") return HELLO;
      if (overrides.has(method)) {
        const overridden = overrides.get(method)(params);
        if (overridden !== undefined) return overridden;
      }
      if (method === "changes.subscribe") held.add(params.subscription_id);
      if (method === "changes.unsubscribe") held.delete(params.subscription_id);
      if (method === "board.list") {
        const listed = { items: structuredClone(board) };
        pending.get(method)?.();
        pending.delete(method);
        return listed;
      }
      const change = pending.get(method);
      pending.delete(method);
      change?.();
      return answer(method, params);
    };
    bridge.call = vi.fn(reply);
    return wire;
  };

  /** The rail as the feed hands it out, read from the records the pass wrote. */
  const watchFeed = async () => {
    const feed = await import("../src/core/taskFeed.js");
    let view = null;
    const stop = feed.subscribeFeed((snapshot) => { view = snapshot; });
    await feed.startFeed();
    return {
      agentsOf: (entityId) => view?.items.find((row) => row.run_id === entityId)?.agents.map((agent) => agent.id),
      stop: () => { stop(); feed.stopFeed(); },
    };
  };

  /** A git status the bridge answers from `head.now`. */
  const movingHead = (wire) => {
    const head = { now: "before" };
    wire.answering("git.status", () => ({ head: head.now, status_key: head.now, files: [] }));
    return head;
  };
  const headOf = async (entityId) => (await read(entityId, "status"))?.value.head;
  const cacheSomething = () => cache.writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, { items: [branchItem()] });

  const agentsOf = async (entityId) => (await read(entityId, "row"))?.value.agents.map((agent) => agent.id);
  const withAgents = (...ids) => branchItem({ agents: ids.map((id) => ({ id })) });

  const greet = () => changeEvents.greetBridge(bridge.call, { deviceId: "dev-1" });

  it("asks for nothing until the bridge holds its subscriptions, where there is something cached to show", async () => {
    await cacheSomething();
    subscribingBridge();
    const heard = [];
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      heard.push(`asked ${method}`);
      // A subscribe the bridge is slow to answer: the pass must wait it out.
      if (method === "changes.subscribe") await new Promise((done) => setTimeout(done, 20));
      const answered = await answering(method, params);
      heard.push(`answered ${method}`);
      return answered;
    });
    await greet();
    sync.startCacheSync();
    await settle();
    await new Promise((done) => setTimeout(done, 60));
    await settle();

    const lastSubscribeAnswered = heard.lastIndexOf("answered changes.subscribe");
    expect(lastSubscribeAnswered).toBeGreaterThan(-1);
    expect(heard.indexOf("asked board.list")).toBeGreaterThan(lastSubscribeAnswered);
  });

  it("does not lose an agent added while a restarted pass fills workspace details", async () => {
    const wire = subscribingBridge();
    await greet();
    board = [withAgents("ag-1")];
    App.route = BRANCH_ROUTE;
    sync.startCacheSync();
    await settle();
    expect(wire.held.has("s-inbox")).toBe(true);

    wire.during("git.status", () => {
      board = [withAgents("ag-1", "ag-2")];
      wire.pushRow("run-1");
    });
    sync.startCacheSync(); // what gate.restartCacheReaders does on hand-back
    await settle();

    expect(await agentsOf("run-1")).toEqual(["ag-1", "ag-2"]);
  });

  it("reads at once where the cache holds nothing, and again once its subscriptions land", async () => {
    const wire = subscribingBridge();
    const head = movingHead(wire);
    await greet();
    board = [branchItem()];
    const answerSubscribes = wire.hold("changes.subscribe");
    sync.startCacheSync();
    await settle();

    expect(calls("board.list")).toHaveLength(1);
    expect(await headOf("run-1")).toBe("before");

    // Changed before the bridge was recording anything for this device.
    head.now = "after";
    answerSubscribes();
    await settle();

    expect(await headOf("run-1")).toBe("after");
    expect(calls("board.list")).toHaveLength(2);
  });

  it("reads again once a refused subscription is taken on", async () => {
    const wire = subscribingBridge();
    const head = movingHead(wire);
    let refusing = true;
    wire.answering("changes.subscribe", (params) => {
      if (refusing && params.subscription_id === "s-background") throw Object.assign(new Error("busy"), { code: "busy" });
    });
    await greet();
    board = [branchItem()];
    sync.startCacheSync();
    await settle();
    expect(wire.held.has("s-background")).toBe(false);

    head.now = "after";
    refusing = false;
    await flush([{ entity_id: "run-1", state: branchItem() }]); // any delivery asks for the diff again
    expect(wire.held.has("s-background")).toBe(true);

    expect(await headOf("run-1")).toBe("after");
  });

  it("reads again when a subscribe answer outlasts the wait", async () => {
    await cacheSomething();
    const wire = subscribingBridge();
    const head = movingHead(wire);
    await greet();
    board = [branchItem()];
    const answerInbox = wire.hold("changes.subscribe", (params) => params.subscription_id === "s-inbox");
    sync.startCacheSync();
    await settle();
    expect(calls("board.list")).toHaveLength(0);

    await new Promise((done) => setTimeout(done, sync.GREETING_WAIT_MS + 300));
    await settle();
    expect(calls("board.list")).toHaveLength(1);
    // The git subscription is still queued behind the inbox's: nobody records this.
    head.now = "after";
    answerInbox();
    await settle();

    expect(await headOf("run-1")).toBe("after");
  }, 30000);

  it("does not let the board it read overwrite a row pushed while the lists were out", async () => {
    const wire = subscribingBridge();
    await greet();
    board = [withAgents("ag-1")];
    // board.list has answered with one agent; the second lands before the
    // project list does, and is pushed at once.
    wire.during("project.list", () => {
      board = [withAgents("ag-1", "ag-2")];
      wire.pushRow("run-1");
    });
    sync.startCacheSync();
    await settle();

    expect(await agentsOf("run-1")).toEqual(["ag-1", "ag-2"]);
  });

  it("paints the row a push wrote while the lists were out, not the board's older copy", async () => {
    const wire = subscribingBridge();
    await greet();
    board = [withAgents("ag-1")];
    const answerProjects = wire.hold("project.list");
    const rail = await watchFeed();
    try {
      sync.startCacheSync();
      await settle();
      board = [withAgents("ag-1", "ag-2")];
      wire.pushRow("run-1");
      await settle();
      answerProjects();
      await settle();

      expect(rail.agentsOf("run-1")).toEqual(["ag-1", "ag-2"]);
    } finally {
      rail.stop();
    }
  });

  it("paints the entity a push put back after taking it away while the lists were out", async () => {
    const wire = subscribingBridge();
    await greet();
    board = [withAgents("ag-1")];
    const answerProjects = wire.hold("project.list");
    const rail = await watchFeed();
    try {
      sync.startCacheSync();
      await settle();
      await flush([{ entity_id: "board", state: { revision: 1, removed: ["run-1"] } }]);
      board = [withAgents("ag-2")];
      wire.pushRow("run-1");
      await settle();
      answerProjects();
      await settle();

      expect(await agentsOf("run-1")).toEqual(["ag-2"]);
      expect(rail.agentsOf("run-1")).toEqual(["ag-2"]);
    } finally {
      rail.stop();
    }
  });

  it("writes nothing back under a workspace a push took away while the pass was reading it", async () => {
    const wire = subscribingBridge();
    await greet();
    board = [branchItem()];
    const answerStatus = wire.hold("git.status");
    sync.startCacheSync();
    await settle();

    await flush([{ entity_id: "board", state: { revision: 1, removed: ["run-1"] } }]);
    answerStatus();
    await settle();

    expect(await cache.cachedAddresses({ deviceId: "dev-1", entityId: "run-1" })).toEqual([]);
    expect(calls("fs.tree")).toHaveLength(0);
  });

  it("writes nothing back under a workspace a push took away while a pushed change was being read", async () => {
    const wire = subscribingBridge();
    wire.answering("fs.read", (params) => ({ path: params.path, size: 3, content_b64: "bmV3" }));
    await greet();
    board = [branchItem()];
    sync.startCacheSync();
    await settle();
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "src/a.js" }, { file: { path: "src/a.js" }, openedAt: 1 });

    // The reader's open file changed: its body is read again.
    const answerFile = wire.hold("fs.read");
    await flush([{ entity_id: "run-1", files: { paths: ["src/a.js"] } }], "s-background");
    expect(calls("fs.read")).toHaveLength(1);
    await flush([{ entity_id: "board", state: { revision: 1, removed: ["run-1"] } }]);
    answerFile();
    await settle();

    expect(await cache.cachedAddresses({ deviceId: "dev-1", entityId: "run-1" })).toEqual([]);
  });

  it("writes nothing back from a push read the sync layer stood down under", async () => {
    const wire = subscribingBridge();
    wire.answering("fs.read", (params) => ({ path: params.path, size: 3, content_b64: "bmV3" }));
    await greet();
    board = [branchItem()];
    sync.startCacheSync();
    await settle();
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "src/a.js" }, { file: { path: "src/a.js" }, openedAt: 1 });

    const answerFile = wire.hold("fs.read");
    await flush([{ entity_id: "run-1", files: { paths: ["src/a.js"] } }], "s-background");
    expect(calls("fs.read")).toHaveLength(1);
    await flush([{ entity_id: "board", state: { revision: 1, removed: ["run-1"] } }]);
    expect(await cache.cachedAddresses({ deviceId: "dev-1", entityId: "run-1" })).toEqual([]);
    // Hand-back: the same device and session, a fresh sync layer.
    board = [];
    sync.startCacheSync();
    await settle();
    answerFile();
    await settle();

    expect(await cache.cachedAddresses({ deviceId: "dev-1", entityId: "run-1" })).toEqual([]);
  });

  it("does not let the project list it read overwrite one a board item carried since", async () => {
    const wire = subscribingBridge();
    await greet();
    board = [branchItem()];
    wire.during("workspace.list", () => {
      changeEvents.dispatchChangeEvent({
        type: "changes",
        subscription_id: "s-inbox",
        items: [{ entity_id: "board", state: { revision: 3, projects: [{ project_id: "p2", name: "relaydb" }] } }],
      }, "dev-1");
    });
    sync.startCacheSync();
    await settle();

    expect((await read("", "projects")).value.map((project) => project.id)).toEqual(["p2"]);
  });

  it("keeps a workspace a push added after the board was read", async () => {
    const wire = subscribingBridge();
    await greet();
    board = [branchItem()];
    wire.during("git.status", () => {
      board = [branchItem(), branchItem({ run_id: "run-2", branch: "build/signup", worktree_id: "wt-2" })];
      wire.pushRow("run-2");
    });
    sync.startCacheSync();
    await settle();

    expect((await read("run-2", "row"))?.value.branch).toBe("build/signup");
  });
});
