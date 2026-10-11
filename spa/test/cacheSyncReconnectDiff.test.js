// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { called, painted, passesSettled } from "./waits.js";
import { patchFor } from "./gitWireFixture.js";

const App = { route: {}, devices: [{ id: "dev-1" }] };
let context, announce;
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: (id) => id === "dev-1" ? context : null,
  liveContexts: () => [context],
  onDeviceStateChanged: (fn) => { announce = fn; return () => {}; },
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] } }),
  subscriptionsSettledFor: async () => {},
  onSubscriptionHeld: () => () => {},
  watchChanges: () => ({ dispose() {} }),
}));

const rows = [1, 2, 3].map((number) => ({
  kind: "branch", project_id: "p", branch: `build/${number}`, state: "building",
  run_id: number === 2 ? null : `run-${number}`, worktree_id: `wt-${number}`,
  agents: [{ id: `ag-${number}` }], last_activity: new Date().toISOString(),
}));
const entityOf = (number) => number === 2 ? "wt-2" : `run-${number}`;
const address = (number) => ({ deviceId: "dev-1", entityId: entityOf(number), kind: "diff" });
const routeTo = (number) => ({ name: "branch", deviceId: "dev-1", projectId: "p", branch: `build/${number}` });
const diffOf = (key, path = `${key}.js`) => ({
  diff_key: key, patch: patchFor(path, key), files: [{ path, additions: 1, deletions: 1 }], commentable: true,
});
const gate = () => { let release; const promise = new Promise((done) => { release = done; }); return { promise, release }; };
let cache, sync, visible, rpc, diffAnswer, disposers, board;
const diffCalls = () => rpc.mock.calls.filter(([method]) => method === "run.diff" || method === "worktree.diff");
const finish = () => passesSettled(sync, "dev-1");
const boot = async () => { sync.startCacheSync(); await finish(); };
const reconnect = async (kind) => {
  if (kind === "path recovery") await sync.syncRestoredDevice("dev-1", context.session);
  else { context.session = {}; context.greeted = Promise.resolve(); announce(); }
  await finish();
};
const holdDiffs = async () => {
  for (const number of [1, 2, 3]) await cache.writeCached(address(number), diffOf(`held-${number}`));
};
const register = (number, active = () => true, deviceId = "dev-1") =>
  disposers.push(visible.registerSyncDiff({ ...address(number), deviceId }, active));

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  document.body.innerHTML = "";
  disposers = [];
  board = { items: rows };
  App.route = routeTo(1);
  diffAnswer = (params) => params.if_diff_key ? { unchanged: true, diff_key: params.if_diff_key } : diffOf("cold");
  rpc = vi.fn(async (method, params = {}) => {
    if (method === "board.list") return board;
    if (method === "project.list") return { projects: [{ project_id: "p" }] };
    if (method === "workspace.list") return { workspaces: [] };
    if (method === "run.diff" || method === "worktree.diff") return diffAnswer(params);
    if (method === "git.status") return { status_key: "status", files: [] };
    if (method === "git.log") return { commits: [] };
    if (method === "git.unpushed") return { commits: [], base: {} };
    if (method === "thread.page") return { items: [], has_more: false };
    if (method === "fs.tree") return { path: params.path, entries: [] };
    if (method === "term.list") return { terminals: [] };
    if (method === "tasks.list") return { tasks: [] };
    if (method === "tasks.columns") return { columns: [] };
    return {};
  });
  context = { deviceId: "dev-1", rpc, session: {}, greeted: Promise.resolve(), active: () => true,
    cacheScope: { deviceId: "dev-1", active: () => true } };
  cache = await import("../src/core/localCache.js");
  context.cacheScope = (await import("../src/core/cacheScope.js")).scopeFor("dev-1");
  sync = await import("../src/core/cacheSync.js");
  visible = await import("../src/core/syncVisibleScopes.js");
});

afterEach(async () => {
  sync.stopCacheSync();
  disposers.forEach((dispose) => dispose());
  await finish();
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});

describe("held working diffs across reconnect", () => {
  it.each(["new session", "path recovery"])("repaints an off-route mounted Changes pane after %s", async (kind) => {
    await holdDiffs();
    await boot();
    const { createReviewPlug } = await import("../src/core/changesReview.js");
    const host = document.createElement("div");
    document.body.append(host);
    const fetchDiff = vi.fn(async () => ({ unchanged: true, diff_key: "held-2", commentable: true }));
    const plug = createReviewPlug({ cacheScope: context.cacheScope, cacheEntity: "wt-2", fetchDiff });
    disposers.push(plug.unmount);
    plug.mount(host);
    await painted(() => host.textContent.includes("held-2.js"), host);
    rpc.mockClear();
    diffAnswer = () => ({ diff_key: "changed", files: [{ path: "changed-during-gap.js", additions: 2, deletions: 0 }] });
    await reconnect(kind);
    expect(diffCalls().map(([method, params]) => [method, params])).toEqual([
      ["run.diff", { run_id: "run-1", patch: false, if_diff_key: "held-1" }],
      ["worktree.diff", { project_id: "p", worktree_id: "wt-2", patch: false, if_diff_key: "held-2" }],
    ]);
    await painted(() => host.textContent.includes("changed-during-gap.js"), host);
    expect(host.textContent).not.toContain("held-2.js");
    expect((await cache.readCached(address(2))).value).toMatchObject({ diff_key: "changed", commentable: true });
    expect((await cache.readCached(address(2))).value.patch).toBeUndefined();
    expect(fetchDiff).not.toHaveBeenCalled();
  });

  it.each([1, 2])("validates routed entity %s without a Changes pane in foreground before the inbox", async (number) => {
    App.route = routeTo(number);
    await holdDiffs();
    await boot();
    expect(diffCalls()).toHaveLength(1);
    const [method, params, envelope] = diffCalls()[0];
    expect(method).toBe(number === 1 ? "run.diff" : "worktree.diff");
    expect(params).toMatchObject({ patch: false, if_diff_key: `held-${number}` });
    expect(envelope?.priority).not.toBe("background");
    const diffIndex = rpc.mock.calls.findIndex(([name]) => name === method);
    const otherThread = rpc.mock.calls.findIndex(([name, params]) => name === "thread.page" && params.agent_id !== `ag-${number}`);
    expect(diffIndex).toBeLessThan(otherThread);
  });

  it("deduplicates all shown panes and excludes other devices, hidden and disposed panes", async () => {
    await holdDiffs();
    register(1);
    register(1);
    register(2);
    register(3, () => false);
    register(3, () => true, "dev-2");
    visible.registerSyncDiff(address(3))();
    await boot();
    expect(diffCalls().map(([, params]) => params.if_diff_key)).toEqual(["held-1", "held-2"]);
  });

  it.each([{ name: "inbox" }, { name: "branch", deviceId: "dev-2", projectId: "p", branch: "build/1" }])(
    "validates mounted panes even when the route is %j", async (route) => {
      App.route = route;
      await holdDiffs();
      register(2);
      register(3);
      await boot();
      expect(diffCalls().map(([, params]) => params.if_diff_key)).toEqual(["held-2", "held-3"]);
    },
  );

  it("keeps accepted bodies on unchanged answers and adds no recovery reads to ordinary passes", async () => {
    await holdDiffs();
    register(2);
    const before = await cache.readCached(address(2));
    await boot();
    expect(await cache.readCached(address(2))).toEqual(before);
    rpc.mockClear();
    await sync.syncDevice("dev-1");
    expect(diffCalls()).toHaveLength(0);
    document.dispatchEvent(new Event("visibilitychange"));
    await finish();
    expect(diffCalls()).toHaveLength(0);
    await reconnect("new session");
    expect(diffCalls()).toHaveLength(2);
  });

  it("keeps a newer ordinary cache write when a mounted recovery response arrives late", async () => {
    await holdDiffs();
    register(2);
    const response = gate();
    diffAnswer = (params) => params.worktree_id ? response.promise : { unchanged: true };
    sync.startCacheSync();
    await called(rpc, () => diffCalls().some(([method]) => method === "worktree.diff"));
    const newer = diffOf("newer-surface");
    try { await cache.writeCached(address(2), newer); }
    finally { response.release({ diff_key: "older-read", files: [{ path: "older.js" }] }); await finish(); }
    expect((await cache.readCached(address(2))).value).toEqual(newer);
  });

  it("validates a mounted run omitted from the inbox and background warming order", async () => {
    board = { items: [rows[0]], runs: [rows[1], rows[2]] };
    await holdDiffs();
    register(2);
    register(3);
    await boot();
    expect(diffCalls().map(([, params]) => params.if_diff_key)).toEqual(["held-1", "held-2", "held-3"]);
  });

  it("preserves the held body and comment state when a recovery read refuses", async () => {
    await holdDiffs();
    register(2);
    const before = await cache.readCached(address(2));
    diffAnswer = () => { throw new Error("path unavailable"); };
    await boot();
    expect(await cache.readCached(address(2))).toEqual(before);
  });

  it("keeps a pending anchored comment while reconnect updates the mounted Changes pane", async () => {
    await holdDiffs();
    await boot();
    const { createReviewPlug } = await import("../src/core/changesReview.js");
    const host = document.createElement("div");
    document.body.append(host);
    const plug = createReviewPlug({ cacheScope: context.cacheScope, cacheEntity: "wt-2",
      fetchDiff: async () => ({ unchanged: true, diff_key: "held-2", commentable: true }), submit: async () => {} });
    disposers.push(plug.unmount);
    plug.mount(host);
    await painted(() => host.querySelector(".fcmt"), host);
    host.querySelector(".fcmt").click();
    await painted(() => document.querySelector(".cp-input"));
    document.querySelector(".cp-input").value = "Keep this anchored comment";
    document.querySelector(".cp-save").click();
    await painted(() => host.querySelector(".pcomment"), host);
    diffAnswer = () => ({ diff_key: "changed", files: [{ path: "held-2.js", additions: 2, deletions: 0 }] });
    await reconnect("new session");
    expect((await cache.readCached(address(2))).value.diff_key).toBe("changed");
    expect(plug.commentOffer().pending).toBe(1);
    expect(host.querySelector(".pcomment").textContent).toContain("Keep this anchored comment");
  });

  it("fences a mounted diff reply from an old recovered path", async () => {
    await holdDiffs();
    register(2);
    await boot();
    rpc.mockClear();
    const response = gate();
    diffAnswer = (params) => params.worktree_id ? response.promise : { unchanged: true };
    const oldPass = sync.syncRestoredDevice("dev-1", context.session);
    await called(rpc, () => diffCalls().some(([method]) => method === "worktree.diff"));
    diffAnswer = () => ({ diff_key: "new-path", files: [{ path: "new-path.js" }] });
    try {
      await sync.syncRestoredDevice("dev-1", context.session);
      expect((await cache.readCached(address(2))).value.diff_key).toBe("new-path");
    } finally { response.release({ diff_key: "old-path", files: [{ path: "old-path.js" }] }); await oldPass; }
    expect((await cache.readCached(address(2))).value.diff_key).toBe("new-path");
  });
});
