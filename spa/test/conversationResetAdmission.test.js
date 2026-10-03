// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const App = { route: { name: "inbox" }, devices: [{ id: "reset-device" }] };
const contexts = new Map();
const watchers = [];
vi.mock("../src/appState.js", async () => ({ App: (await import("../src/app.js")).App }));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: (id) => contexts.get(id), liveContexts: () => [...contexts.values()], onDeviceStateChanged: () => () => {},
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] } }),
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: (watcher) => { watchers.push(watcher); return { dispose() {} }; },
}));

let cache, sync, board;
const address = (kind, sub = "") => ({ deviceId: "reset-device", entityId: "run-1", kind, sub });
const agent = (threadId, revision, extra = {}) => ({ id: "ag-1", conversation_id: "ag-1", thread_id: threadId, thread_generation_revision: revision, ...extra });
const row = (agents) => ({ kind: "branch", run_id: "run-1", project_id: "p1", worktree_id: "wt-1", branch: "build/reset", state: "building", agents });
const nextTurn = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  contexts.clear(); watchers.length = 0;
  board = [];
  const context = {
    deviceId: "reset-device", session: {}, greeted: Promise.resolve(), cacheScope: { active: () => true }, active: () => true,
    rpc: async (method) => {
      if (method === "board.list") return { items: board };
      if (method === "project.list") return { projects: [] };
      if (method === "workspace.list") return { workspaces: [] };
      if (method === "thread.page") return { thread_id: "thread:ag-1", items: [], has_more: false };
      return {};
    },
  };
  contexts.set("reset-device", context);
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/cacheSync.js");
  expect((await import("../src/appState.js")).App).toBe(App);
  sync.startCacheSync();
  await vi.waitFor(() => expect(watchers.some((watcher) => watcher.id === "s-inbox")).toBe(true));
  for (let i = 0; i < 10; i += 1) await nextTurn();
  await cache.writeCached(address("thread", "ag-1"), { thread_id: "thread:ag-1", items: [], deliveredSequence: 0 });
});

afterEach(() => { sync.stopCacheSync(); vi.restoreAllMocks(); });

async function remoteReset() {
  await cache.writeCached(address("thread", "ag-1"), {
    thread_id: "thread:ag-1:fresh", items: [], deliveredSequence: 0, thread_generation_revision: 1, retired_thread_ids: ["thread:ag-1"],
  });
  await cache.writeCached(address("row"), row([agent("thread:ag-1:fresh", 1)]));
  await cache.deleteCached([address("surfaces", "ag-1")]);
}

/** Pause the old candidate immediately before its database write, while a
 * separate client advances the shared thread generation. */
function pauseAdmission(kind) {
  let release;
  const pause = (run) => new Promise((resolve, reject) => { release = () => run().then(resolve, reject); });
  for (const name of ["mergeCachedAtomically", "mergeCachedTogether", "mergeCachedRecordsTogether", "updateCachedFeed", "writeCached"]) {
    const original = cache[name];
    vi.spyOn(cache, name).mockImplementation((at, ...args) => {
      const target = Array.isArray(at) ? at : [at];
      if (!release && target.some((held) => held.kind === kind && (kind === "feed" || held.entityId === "run-1"))) return pause(() => original(at, ...args));
      return original(at, ...args);
    });
  }
  return {
    waiting: () => typeof release === "function",
    resume: () => release(),
  };
}

const pushed = (state) => watchers.find((watcher) => watcher.id === "s-inbox").onChanges([{ entity_id: "run-1", state }]);

describe("conversation generation admission across clients", () => {
  it("scrubs a delayed board feed against the current thread while keeping unrelated rows", async () => {
    const stale = { ...row([agent("thread:ag-1", 0, { topic: "old secret topic" })]), summary: "old secret summary" };
    const sibling = { ...row([]), run_id: "run-2", summary: "keep sibling" };
    board = [stale, sibling];
    const paused = pauseAdmission("feed");
    const reading = sync.syncDevice("reset-device");
    await vi.waitFor(() => expect(paused.waiting()).toBe(true));
    await remoteReset(); paused.resume(); await reading;
    const feed = (await cache.readCached({ ...address("feed"), entityId: "" })).value;
    expect(JSON.stringify(feed)).not.toContain("old secret");
    expect(feed.items.find((held) => held.run_id === "run-2").summary).toBe("keep sibling");
    expect(feed.items.find((held) => held.run_id === "run-1").agents[0].thread_id).toBe("thread:ag-1:fresh");
  });

  it("refuses an old listed row when reset commits after reconciliation", async () => {
    board = [row([agent("thread:ag-1", 0)])];
    const paused = pauseAdmission("row");
    const reading = sync.syncDevice("reset-device");
    await vi.waitFor(() => expect(paused.waiting()).toBe(true));
    await remoteReset(); paused.resume(); await reading;
    expect((await cache.readCached(address("row"))).value.agents[0].thread_id).toBe("thread:ag-1:fresh");
  });

  it("refuses an old pushed row when reset commits after reconciliation", async () => {
    const paused = pauseAdmission("row");
    pushed(row([agent("thread:ag-1", 0)]));
    await vi.waitFor(() => expect(paused.waiting()).toBe(true));
    await remoteReset(); paused.resume();
    for (let i = 0; i < 10; i += 1) await nextTurn();
    expect((await cache.readCached(address("row"))).value.agents[0].thread_id).toBe("thread:ag-1:fresh");
  });

  it("refuses an old surface snapshot when reset commits after its thread check", async () => {
    const paused = pauseAdmission("surfaces");
    pushed(row([agent("thread:ag-1", 0, { surface_session_generation: "old-session", surfaces: { goal: "old secret" } })]));
    await vi.waitFor(() => expect(paused.waiting()).toBe(true));
    await remoteReset(); paused.resume();
    for (let i = 0; i < 10; i += 1) await nextTurn();
    expect(await cache.readCached(address("surfaces", "ag-1"))).toBeUndefined();
  });

  it("replaces an empty pooled session on reset and refuses a later retired tip", async () => {
    const projects = { deviceId: "reset-device", entityId: "", kind: "projects", sub: "" };
    await cache.writeCached(projects, [{ project_id: "p1", name: "Build", session_started_ms: 100, last_activity_ms: 200 }]);
    const tip = (threadId, revision, session) => ({
      agent_id: "ag-1", conversation_id: "ag-1", thread_id: threadId, thread_generation_revision: revision,
      project_id: "p1", project_session: session, last_sequence: 0,
    });
    const watcher = watchers.find((held) => held.id === "s-inbox");
    watcher.onChanges([{ entity_id: "run-1", thread: [tip("thread:ag-1:fresh", 1, { session_started_ms: null, last_activity_ms: null })] }]);
    await vi.waitFor(async () => expect((await cache.readCached(projects)).value[0].last_activity_ms).toBe(null));
    watcher.onChanges([{ entity_id: "run-1", thread: [tip("thread:ag-1", 0, { session_started_ms: 100, last_activity_ms: 200 })] }]);
    for (let i = 0; i < 10; i += 1) await nextTurn();
    expect((await cache.readCached(projects)).value[0]).toMatchObject({ session_started_ms: null, last_activity_ms: null });
  });
});
