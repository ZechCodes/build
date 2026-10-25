// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { called, passesSettled } from "./waits.js";

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
let watchers, contexts, deviceChanged;
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, projectSources: true,
    kinds: ["state", "thread", "git", "files", "terminals", "tasks"] }, term: { projectSources: true } }),
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: (registration) => { watchers.push(registration); return { dispose() {} }; },
}));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: (id) => contexts.find((context) => context.deviceId === id), liveContexts: () => contexts.filter((context) => context.active()),
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
  onDeviceStateChanged: (callback) => { deviceChanged = callback; return () => {}; },
}));
let sync, cache;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  watchers = []; contexts = [makeContext("dev-1")];
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/cacheSync.js");
});
afterEach(() => { sync.stopCacheSync(); vi.restoreAllMocks(); });

function makeContext(deviceId) {
  const context = { deviceId, session: {}, greeted: Promise.resolve(), connected: true };
  context.active = () => context.connected;
  context.cacheScope = { deviceId, active: context.active };
  context.rpc = vi.fn(async (method, params = {}) => {
    const answers = {
      "board.list": { items: [{ kind: "branch", state: "building", project_id: "p1", run_id: "run-1", worktree_id: "wt-1",
        agents: [{ id: "agent-1", conversation_id: "c1", thread_id: "thread:c1" }] }] },
      "project.list": { projects: [{ project_id: "p1", sources: [] }] }, "workspace.list": { workspaces: [] },
      "thread.page": { thread_id: params.thread_id, items: [], has_more: false },
      "term.list": { terminals: [] }, "tasks.list": { tasks: [] }, "tasks.columns": { columns: [] },
      "fs.tree": { entries: [] }, "git.status": { files: [], status_key: "initial" },
      "git.log": { commits: [] }, "git.unpushed": { commits: [] },
    };
    return answers[method] || {};
  });
  return context;
}

it("announces its current boolean immediately and supports unsubscribe", () => {
  const listener = vi.fn();
  const unsubscribe = sync.subscribeConnectionSync(listener);
  expect(listener.mock.calls).toEqual([[false]]);
  unsubscribe();
  sync.startCacheSync();
  expect(listener.mock.calls).toEqual([[false]]);
});

function gate() {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release };
}
function hold(context, method, predicate = () => true) {
  const held = gate(), previous = context.rpc.getMockImplementation();
  context.rpc.mockImplementation((name, params, ...rest) => name === method && predicate(params)
    ? held.promise : previous(name, params, ...rest));
  return held;
}
const signals = () => {
  const listener = vi.fn();
  sync.subscribeConnectionSync(listener);
  return { listener, values: () => listener.mock.calls.map(([active]) => active) };
};
const push = (items) => watchers.find((entry) => entry.kinds.includes("thread")).onChanges(items);
const started = async (context, method = "board.list") => called(context.rpc,
  () => context.rpc.mock.calls.some(([name]) => name === method));
const idle = async (state) => {
  await passesSettled(sync, "dev-1");
  await vi.waitFor(() => expect(state.values().at(-1)).toBe(false));
};

it("stays active through the post-connect pass and announces idle when it completes", async () => {
  const state = signals(), context = contexts[0], held = hold(context, "board.list");
  sync.startCacheSync();
  await started(context);
  expect(state.values()).toEqual([false, true]);
  held.release({ items: [] });
  await idle(state);
  expect(state.values()).toEqual([false, true, false]);
});

it("keeps a chained pass active without an idle notification between passes", async () => {
  const state = signals(), context = contexts[0], held = hold(context, "board.list");
  sync.startCacheSync();
  await started(context);
  const trailing = sync.syncDevice("dev-1");
  held.release({ items: [] });
  await trailing;
  await idle(state);
  expect(context.rpc.mock.calls.filter(([method]) => method === "board.list")).toHaveLength(2);
  expect(state.values()).toEqual([false, true, false]);
});

it("waits until all connected devices finish their post-connect passes", async () => {
  const second = makeContext("dev-2"); contexts.push(second);
  const firstHeld = hold(contexts[0], "board.list"), secondHeld = hold(second, "board.list"), state = signals();
  sync.startCacheSync();
  await Promise.all(contexts.map((context) => started(context)));
  firstHeld.release({ items: [] });
  await passesSettled(sync, "dev-1");
  expect(state.values()).toEqual([false, true]);
  secondHeld.release({ items: [] });
  await passesSettled(sync, "dev-2");
  expect(state.values()).toEqual([false, true, false]);
});

it("does not restart for later pushes, manual sync, or visibility refresh", async () => {
  const state = signals();
  sync.startCacheSync(); await idle(state);
  await push([{ entity_id: "p1", tasks: { task_ids: ["updated"] } }]);
  await sync.syncDevice("dev-1");
  document.dispatchEvent(new Event("visibilitychange"));
  await passesSettled(sync, "dev-1");
  expect(state.values()).toEqual([false, true, false]);
});

it("holds active for push enrichment still running after the pass completes", async () => {
  const state = signals(), context = contexts[0], pass = hold(context, "board.list");
  const work = hold(context, "tasks.list", (params) => params.project_id.startsWith("pushed-project"));
  sync.startCacheSync(); await started(context);
  const pushed = push([{ entity_id: "pushed-project", tasks: { task_ids: ["updated"] } }]);
  await started(context, "tasks.list");
  pass.release({ items: [] }); await passesSettled(sync, "dev-1");
  expect(state.values()).toEqual([false, true]);
  const queued = [1, 2].map((index) => push([{ entity_id: `pushed-project-${index}`, tasks: { task_ids: ["updated"] } }]));
  await vi.waitFor(() => expect(sync.pushWorkInFlight("dev-1").backgroundPending).toBeGreaterThan(0));
  expect(state.values()).toEqual([false, true]);
  work.release({ tasks: [] }); await Promise.all([pushed, ...queued]); await idle(state);
  expect(state.values()).toEqual([false, true, false]);
});

it("holds active for thread catch-up after body application and the pass finish", async () => {
  const state = signals(), context = contexts[0], pass = hold(context, "board.list");
  const reading = hold(context, "thread.page");
  await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "c1" },
    { thread_id: "thread:c1", items: [], deliveredSequence: 0 });
  sync.startCacheSync(); await started(context);
  const pushed = push([{ entity_id: "run-1", thread: [{ agent_id: "agent-1", conversation_id: "c1",
    thread_id: "thread:c1", since_sequence: 5, last_sequence: 10, items: [] }] }]);
  await started(context, "thread.page");
  pass.release({ items: [] }); await passesSettled(sync, "dev-1");
  expect(sync.threadCatchupsInFlight("dev-1").running).toBe(1);
  expect(state.values()).toEqual([false, true]);
  reading.release({ thread_id: "thread:c1", thread_last_sequence: 10, has_more: false,
    items: [{ type: "message", data: { id: "m10", sequence: 10 } }] });
  await pushed; await idle(state);
});

it("does not treat retained repair debt in the cache as running work", async () => {
  const state = signals();
  const address = { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "c1" };
  await cache.writeCached(address, { thread_id: "thread:c1", items: [], deliveredSequence: 10, repairThrough: 10 });
  const context = contexts[0], previous = context.rpc.getMockImplementation();
  context.rpc.mockImplementation((method, ...args) => {
    if (method === "thread.page") throw new Error("offline");
    return previous(method, ...args);
  });
  sync.startCacheSync(); await idle(state);
  expect((await cache.readCached(address))?.value.repairThrough).toBe(10);
  expect(state.values()).toEqual([false, true, false]);
});

it("clears a disconnected device immediately and starts again for its new session", async () => {
  const state = signals(), context = contexts[0], held = hold(context, "board.list");
  sync.startCacheSync(); await started(context);
  context.connected = false; deviceChanged();
  expect(state.values()).toEqual([false, true, false]);
  held.release({ items: [] }); await passesSettled(sync, "dev-1");
  context.connected = true; context.session = {}; deviceChanged();
  await idle(state);
  expect(state.values()).toEqual([false, true, false, true, false]);
});

it("clears the signal on stop and ignores completion from the stopped lifetime", async () => {
  const state = signals(), context = contexts[0], held = hold(context, "board.list");
  sync.startCacheSync(); await started(context);
  const oldPass = sync.passInFlight("dev-1");
  sync.stopCacheSync();
  expect(state.values()).toEqual([false, true, false]);
  held.release({ items: [] }); await oldPass;
  expect(state.values()).toEqual([false, true, false]);
});

it("does not reactivate when a queued replacement disconnects before its pass starts", async () => {
  const state = signals(), context = contexts[0], held = hold(context, "board.list");
  sync.startCacheSync(); await started(context);
  const oldPass = sync.passInFlight("dev-1");
  context.session = {}; deviceChanged();
  const replacement = sync.syncDevice("dev-1");
  context.connected = false; deviceChanged();
  expect(state.values()).toEqual([false, true, false]);
  held.release({ items: [] });
  await Promise.all([oldPass, replacement]);
  expect(state.values()).toEqual([false, true, false]);
});

it("starts a new active period when an existing session restores its connection", async () => {
  const state = signals(), context = contexts[0];
  sync.startCacheSync(); await idle(state);
  const held = hold(context, "board.list");
  const restored = sync.syncRestoredDevice("dev-1", context.session);
  expect(state.values()).toEqual([false, true, false, true]);
  held.release({ items: [] }); await restored;
  expect(state.values()).toEqual([false, true, false, true, false]);
});

async function holdPostConnectWork(kind, beforeFinish = async () => {}) {
  const state = signals(), context = contexts[0], pass = hold(context, "board.list");
  const method = kind === "enrichment" ? "tasks.list" : "thread.page";
  const reading = hold(context, method, (params) => kind === "catch-up" || params.project_id === "pushed-project");
  sync.startCacheSync(); await started(context);
  const items = kind === "enrichment"
    ? [{ entity_id: "pushed-project", tasks: { task_ids: ["updated"] } }]
    : [{ entity_id: "run-1", thread: [{ agent_id: "agent-1", conversation_id: "c1",
      thread_id: "thread:c1", since_sequence: 5, last_sequence: 10, items: [] }] }];
  const pending = push(items);
  await started(context, method);
  await beforeFinish(context);
  pass.release({ items: [] }); await passesSettled(sync, "dev-1");
  expect(state.values()).toEqual([false, true]);
  return { context, state, pending, release: () => reading.release(kind === "enrichment" ? { tasks: [] }
    : { thread_id: "thread:c1", thread_last_sequence: 10, has_more: false,
      items: [{ type: "message", data: { id: "m10", sequence: 10 } }] }) };
}

async function replaceConnection(operation, context) {
  if (operation === "restore") return sync.syncRestoredDevice("dev-1", context.session);
  if (operation === "new session") {
    context.session = {}; deviceChanged();
  } else if (operation === "restart") {
    sync.stopCacheSync(); sync.startCacheSync();
  } else {
    context.connected = false; contexts = []; deviceChanged();
    contexts = [makeContext("dev-1")]; deviceChanged();
  }
  await passesSettled(sync, "dev-1");
}

it.each(["enrichment", "catch-up"].flatMap((kind) =>
  ["restore", "new session", "restart", "re-adoption"].map((operation) => ({ kind, operation }))))(
  "ignores retired $kind queues after connection $operation before their RPC answers", async ({ kind, operation }) => {
    const work = await holdPostConnectWork(kind);
    const expected = ["restart", "re-adoption"].includes(operation)
      ? [false, true, false, true, false] : [false, true, false];
    try {
      await replaceConnection(operation, work.context);
      expect(work.state.values()).toEqual(expected);
    } finally {
      work.release(); await work.pending;
    }
    expect(work.state.values()).toEqual(expected);
  },
);


it.each(["session request", "restored path"])("clears an abandoned %s when its awaited pass loses the session", async (kind) => {
  const state = signals(), context = contexts[0], held = hold(context, "board.list");
  sync.startCacheSync(); await started(context);
  let requested;
  if (kind === "session request") {
    context.session = {}; deviceChanged();
  } else requested = sync.syncRestoredDevice("dev-1", context.session);
  context.session = {};
  held.release({ items: [] });
  await requested; await passesSettled(sync, "dev-1");
  await vi.waitFor(() => expect(state.values()).toEqual([false, true, false]));
});

it("stops under traffic arriving every tick during and after the pass once its snapshot drains", async () => {
  const pushes = [];
  let tick = 0, afterPass = false, traffic;
  const work = await holdPostConnectWork("enrichment", async () => {
    traffic = setInterval(() => {
      pushes.push(push([{ entity_id: `${afterPass ? "later" : "during"}-${tick++}`, tasks: { task_ids: ["updated"] } }]));
    }, 1);
    await vi.waitFor(() => expect(tick).toBeGreaterThan(2));
  });
  const later = hold(work.context, "tasks.list", (params) => params.project_id.startsWith("later-"));
  try {
    afterPass = true;
    await vi.waitFor(() => expect(work.context.rpc.mock.calls.some(([method, params]) =>
      method === "tasks.list" && params.project_id.startsWith("later-"))).toBe(true));
    expect(work.state.values()).toEqual([false, true]);
    work.release(); await work.pending;
    await vi.waitFor(() => expect(work.state.values()).toEqual([false, true, false]));
    expect(sync.pushWorkInFlight("dev-1").backgroundRunning).toBeGreaterThan(0);
  } finally {
    clearInterval(traffic); later.release({ tasks: [] });
    await Promise.all(pushes);
  }
});

it("keeps each device's cut bounded while another device is still connecting", async () => {
  const second = makeContext("dev-2"); contexts.push(second);
  const secondPass = hold(second, "board.list");
  const work = await holdPostConnectWork("enrichment");
  const later = hold(work.context, "tasks.list", (params) => params.project_id === "later-device-one");
  const pushedLater = push([{ entity_id: "later-device-one", tasks: { task_ids: ["updated"] } }]);
  await started(work.context, "tasks.list");
  work.release(); await work.pending;
  expect(work.state.values()).toEqual([false, true]);
  try {
    secondPass.release({ items: [] }); await passesSettled(sync, "dev-2");
    await vi.waitFor(() => expect(work.state.values()).toEqual([false, true, false]));
  } finally {
    later.release({ tasks: [] }); await pushedLater;
  }
});
