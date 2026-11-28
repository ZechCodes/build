// @vitest-environment jsdom
// A registry fence is invisible to readers: the real mounted rail and project
// directory continue reading their ordinary cache records throughout recovery.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { deviceRecords, deviceRecord } from "./fixtures/deviceSync.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const deviceId = "device-paint-997";
const scope = { kind: "device" };
const address = (kind) => ({ deviceId, entityId: "", kind });
const project = (name) => ({ ...deviceRecords().find((record) => record.kind === "project").row, name });
const reply = (extra) => ({ status: "ok", scope, incarnation: "incarnation-paint", bridge_epoch: "epoch-paint", ...extra });
const reconciling = () => Object.assign(new Error("sync_reconciling"), { code: "unavailable", retryable: true });
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};

// A retry settles only after its asynchronous cache capture and RPC refusal
// schedule the next backoff. No fixed number of IndexedDB turns establishes it.
async function settledRetry(request, calls, nextDelay, run) {
  const scheduled = deferred(), setTimer = globalThis.setTimeout;
  const timer = vi.spyOn(globalThis, "setTimeout").mockImplementation((callback, delay, ...args) => {
    const handle = setTimer(callback, delay, ...args);
    if (delay === nextDelay && request.mock.calls.length === calls) scheduled.resolve();
    return handle;
  });
  try {
    const result = await run();
    await scheduled.promise;
    expect(request).toHaveBeenCalledTimes(calls);
    return result;
  } finally { timer.mockRestore(); }
}
let cache, feed, inbox, contexts, engine;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  const { App } = await import("../src/app.js");
  Object.assign(App, { route: { name: "inbox" }, devices: [{ id: deviceId, name: "Synthetic device", status: "online" }],
    selectedDeviceId: deviceId, deviceFilter: null });
  cache = await import("../src/core/localCache.js");
  feed = await import("../src/core/taskFeed.js");
  inbox = await import("../src/core/inboxView.js");
  contexts = await import("../src/core/deviceContexts.js");
});

afterEach(() => {
  engine?.dispose(); engine = null;
  inbox?.unmountInboxList(); feed?.stopFeed(); feed?.dropFeedDevice(deviceId);
  contexts?.resetDeviceContexts();
  document.body.innerHTML = "";
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

async function mountCachedRows({ workspaces = [], items = [] } = {}) {
  const { writeRailBoard } = await import("./railCacheFixture.js");
  await writeRailBoard({ projects: [project("Cached project")], workspaces, items }, { deviceId });
  await cache.writeCached(address("device-projects"), [project("Cached project")]);
  inbox.setInboxView("inbox"); inbox.mountInboxList(); await feed.startFeed();
  const { deviceProjectsPanelHtml, mountDeviceProjects } = await import("../src/views/deviceProjects.js");
  const host = document.createElement("section");
  host.id = "device-directory"; host.innerHTML = deviceProjectsPanelHtml(); document.body.appendChild(host);
  // A mounted panel's independent read remains outstanding; its cached rows
  // must paint before this request, or any device-scope request, answers.
  void mountDeviceProjects(host, { deviceId, deviceName: "Synthetic device", callRpc: () => new Promise(() => {}) });
  await vi.waitFor(() => {
    expect(document.querySelector(`#inbox-list [data-key="project-agent:${deviceId}/proj-1"] .stitle`)?.textContent).toBe("Cached project");
    expect(host.querySelector(".pname")?.textContent).toBe("Cached project");
  });
  return host;
}

async function startEngine(request, onError) {
  const { createScopedSyncEngine } = await import("../src/core/scopedSyncEngine.js");
  const { createScopedDeviceSyncCache } = await import("../src/core/scopedDeviceSyncCache.js");
  engine = createScopedSyncEngine({ request, onError, epoch: () => "epoch-paint",
    storeFor: () => createScopedDeviceSyncCache({ deviceId, scope, bridgeIdentity: deviceId }) });
  engine.track(scope);
  return engine.catchUp(scope);
}

it("keeps cached inbox and directory DOM and records identical through a long fence, then paints catch-up", async () => {
  const host = await mountCachedRows();
  const beforeDom = document.querySelector("#inbox-list").innerHTML;
  const beforeDirectory = host.innerHTML;
  const kinds = ["projects", "device-projects", "feed"];
  const beforeCache = await Promise.all(kinds.map((kind) => cache.readCached(address(kind))));
  let fenced = true;
  const onError = vi.fn(), changesRequested = deferred(), changesReply = deferred();
  const request = vi.fn(async (method) => {
    if (fenced) throw reconciling();
    if (method === "sync.snapshot") return reply({ consistency: "leased", lease: { id: "lease-paint", expires_at_ms: Date.now() + 60_000 },
      watermark: "1", cursor: "cursor-paint-1", records: [deviceRecord("project", "proj-1", project("Caught-up project"))] });
    changesRequested.resolve();
    return changesReply.promise;
  });
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  expect(await settledRetry(request, 1, 1000, () => startEngine(request, onError))).toBe(false);
  await settledRetry(request, 2, 2000, () => vi.advanceTimersByTimeAsync(1000));
  await settledRetry(request, 3, 5000, () => vi.advanceTimersByTimeAsync(2000));
  expect(request).toHaveBeenCalledTimes(3);
  expect(onError).not.toHaveBeenCalled();
  expect(await Promise.all(kinds.map((kind) => cache.readCached(address(kind))))).toEqual(beforeCache);
  expect(document.querySelector("#inbox-list").innerHTML).toBe(beforeDom);
  expect(host.innerHTML).toBe(beforeDirectory);
  expect(document.body.textContent).not.toMatch(/sync_reconciling|reconciling|No projects yet|No activity/i);
  fenced = false;
  await vi.advanceTimersByTimeAsync(5000);
  await changesRequested.promise;
  // Join the active recovery while its changes response is deliberately held.
  const caughtUp = engine.catchUp(scope);
  changesReply.resolve(reply({ head: "1", cursor: "cursor-paint-1", changes: [] }));
  expect(await caughtUp).toBe(true);
  vi.useRealTimers();
  await vi.waitFor(() => {
    expect(document.querySelector("#inbox-list .stitle")?.textContent).toBe("Caught-up project");
    expect(host.querySelector(".pname")?.textContent).toBe("Caught-up project");
  });
  expect(onError).not.toHaveBeenCalled();
});

it("reports a persisting genuine failure through the engine's existing error callback", async () => {
  await mountCachedRows();
  const failure = Object.assign(new Error("Synthetic device read failure"), { code: "unavailable", retryable: true });
  const request = vi.fn(async () => { throw failure; });
  const onError = vi.fn();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  expect(await settledRetry(request, 1, 1000, () => startEngine(request, onError))).toBe(false);
  expect(onError).not.toHaveBeenCalled();
  await settledRetry(request, 2, 2000, () => vi.advanceTimersByTimeAsync(1000));
  expect(onError).not.toHaveBeenCalled();
  await settledRetry(request, 3, 5000, () => vi.advanceTimersByTimeAsync(2000));
  expect(request).toHaveBeenCalledTimes(3);
  expect(onError).toHaveBeenCalledWith(failure);
  expect(document.querySelector("#inbox-list .stitle")?.textContent).toBe("Cached project");
});


async function ownDeviceScope(ownedDeviceId = deviceId) {
  vi.stubGlobal("navigator", { locks: { request: vi.fn() } });
  const call = vi.fn(async (method) => method === "session.hello"
    ? { api_version: "3.34.0", push_events: false, capabilities: ["sync.device"] } : {});
  const context = contexts.adoptDeviceSession({ deviceId: ownedDeviceId, call, close() {}, peer() {}, onCarrier() {},
    installAdapter: (selection) => selection.create(call) });
  const { greetLiveBridge } = await import("../src/connection.js");
  await greetLiveBridge(context);
  const { registerScopedSync } = await import("../src/core/scopedSyncRegistry.js");
  const release = registerScopedSync({ deviceId: ownedDeviceId, scope });
  const { deviceScopedSyncOwns } = await import("../src/core/scopedSyncRuntime.js");
  const activeOwner = () => context.active() && deviceScopedSyncOwns(ownedDeviceId);
  expect(activeOwner()).toBe(true);
  return { release, activeOwner };
}

it("paints a runtime pending lifecycle row under device ownership and retains it through a completed snapshot", async () => {
  await mountCachedRows();
  const { release, activeOwner } = await ownDeviceScope();
  try {
    const { enrichDeviceRuntimeBoard } = await import("../src/core/scopedDeviceRuntime.js");
    // The exact board PendingRow shape, from bridge api/v1/board.rs and
    // runtime/deferred.rs. It belongs to a live clone, not device journal rows.
    const pending = { entity_id: "pending-clone", project_id: "proj-1", project: "Cached project",
      title: "Clone workspace", branch: "build/pending-clone", state: "creating", checkout_id: null,
      implements: null, isolation: "worktree", pending_seconds: 1 };
    await enrichDeviceRuntimeBoard(deviceId, { pending: [pending] }, activeOwner);
    const row = () => [...document.querySelectorAll("#inbox-list .inbox-entry")]
      .find((entry) => entry.querySelector(".stitle")?.textContent === "build/pending-clone");
    await vi.waitFor(() => {
      expect(row()?.querySelector(".stitle")?.textContent).toBe("build/pending-clone");
      expect(row()?.querySelector(".inbox-facts")?.textContent).toBe("Creating…");
    });
    const before = (await cache.readCached(address("feed"))).value.pending;
    expect(before).toMatchObject([{ ...pending, deviceId }]);
    const request = vi.fn(async (method) => method === "sync.snapshot"
      ? reply({ consistency: "leased", lease: { id: "lease-runtime", expires_at_ms: Date.now() + 60_000 },
        watermark: "1", cursor: "cursor-runtime", records: [deviceRecord("project", "proj-1", project("Cached project"))] })
      : reply({ head: "1", cursor: "cursor-runtime", changes: [] }));
    const onError = vi.fn();
    expect(await startEngine(request, onError)).toBe(true);
    expect(request.mock.calls.map(([method]) => method)).toEqual(["sync.snapshot", "sync.changes"]);
    expect((await cache.readCached(address("feed"))).value.pending).toEqual(before);
    await vi.waitFor(() => {
      expect(row()?.querySelector(".stitle")?.textContent).toBe("build/pending-clone");
      expect(row()?.querySelector(".inbox-facts")?.textContent).toBe("Creating…");
    });
    expect(activeOwner()).toBe(true);
    expect(onError).not.toHaveBeenCalled();
  } finally { release(); }
});

it("paints identical pending path hashes from two owned devices with distinct stable DOM keys", async () => {
  await mountCachedRows();
  const otherDeviceId = "device-paint-second-997";
  const { App } = await import("../src/app.js");
  App.devices.push({ id: otherDeviceId, name: "Second synthetic device", status: "online" });
  const { writeRailBoard } = await import("./railCacheFixture.js");
  await writeRailBoard({ projects: [project("Second cached project")] }, { deviceId: otherDeviceId });
  await feed.joinFeed({ deviceId: otherDeviceId });
  const first = await ownDeviceScope(), second = await ownDeviceScope(otherDeviceId);
  try {
    const { enrichDeviceRuntimeBoard } = await import("../src/core/scopedDeviceRuntime.js");
    const pending = { entity_id: "same-path-hash", project_id: "proj-1", project: "Cached project", title: "Clone workspace",
      branch: "build/first-device", state: "creating", checkout_id: null, implements: null, isolation: "worktree", pending_seconds: 1 };
    await enrichDeviceRuntimeBoard(deviceId, { pending: [pending] }, first.activeOwner);
    await enrichDeviceRuntimeBoard(otherDeviceId, { pending: [{ ...pending, branch: "build/second-device" }] }, second.activeOwner);
    const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")]
      .filter((row) => row.querySelector(".stitle")?.textContent.startsWith("build/"));
    const distinctRows = () => {
      expect(rows().map((row) => row.querySelector(".stitle").textContent).sort()).toEqual(["build/first-device", "build/second-device"]);
      expect(new Set(rows().map((row) => row.dataset.key)).size).toBe(2);
      expect(rows().every((row) => row.querySelector(".inbox-facts")?.textContent === "Creating…")).toBe(true);
    };
    await vi.waitFor(distinctRows);
    const beforeKeys = rows().map((row) => row.dataset.key).sort();
    feed.deliverFeed();
    await vi.waitFor(distinctRows);
    expect(rows().map((row) => row.dataset.key).sort()).toEqual(beforeKeys);
  } finally { first.release(); second.release(); feed.dropFeedDevice(otherDeviceId); }
});

it("paints a standing workspace operation once and retains its pending facts through a completed snapshot", async () => {
  const workspace = deviceRecords().find((record) => record.kind === "workspace").row;
  const owner = { kind: "branch", entity_id: "owner-1", workspace_id: "workspace-1", project_id: "proj-1", agents: [] };
  await mountCachedRows({ workspaces: [{ ...workspace, entity_id: "owner-1" }], items: [owner] });
  const { release, activeOwner } = await ownDeviceScope();
  try {
    const { enrichDeviceRuntimeBoard } = await import("../src/core/scopedDeviceRuntime.js");
    const pending = { entity_id: "owner-1", project_id: "proj-1", project: "Cached project", title: "Updating workspace",
      branch: "synthetic", state: "updating", checkout_id: "workspace-1", implements: null, isolation: "worktree", pending_seconds: 1 };
    await enrichDeviceRuntimeBoard(deviceId, { pending: [pending] }, activeOwner);
    const row = () => document.querySelector(`#inbox-list .inbox-entry[data-key="workspace:${deviceId}/workspace-1"]`);
    const paintedOnce = () => {
      expect(row()?.querySelector(".stitle")?.textContent).toBe("Synthetic workspace");
      expect(row()?.querySelector(".inbox-facts")?.textContent).toContain("Updating…");
      expect(row()?.querySelector(".inbox-status-running")).not.toBeNull();
      expect(document.querySelectorAll('#inbox-list .inbox-entry[data-key="owner-1"]')).toHaveLength(0);
      expect([...document.querySelectorAll("#inbox-list .stitle")].filter((title) => title.textContent === "synthetic")).toHaveLength(0);
    };
    await vi.waitFor(paintedOnce);
    const before = (await cache.readCached(address("feed"))).value.pending;
    const request = vi.fn(async (method) => method === "sync.snapshot"
      ? reply({ consistency: "leased", lease: { id: "lease-standing", expires_at_ms: Date.now() + 60_000 },
        watermark: "1", cursor: "cursor-standing", records: deviceRecords() })
      : reply({ head: "1", cursor: "cursor-standing", changes: [] }));
    expect(await startEngine(request, vi.fn())).toBe(true);
    expect((await cache.readCached(address("feed"))).value.pending).toEqual(before);
    await vi.waitFor(paintedOnce);
  } finally { release(); }
});

it("finishes a multi-day HTTPS catch-up within the device snapshot lease", async () => {
  const { httpsDeviceSnapshot } = await import("./fixtures/httpsDeviceSnapshot.js");
  const fixture = httpsDeviceSnapshot();
  const session = await fixture.openSession();
  session.allowRefresh();
  try {
    expect(await startEngine(session.rawCall)).toBe(true);
    fixture.advanceThreeDays();
    const [recovered] = await engine.foreground();
    expect({ recovered, ...fixture.counts }).toEqual({ recovered: true, pages: 9, returned: 3501, expiries: 0, overBudget: 1 });
    const last = await cache.readCached({ ...address("sync-device-record"), sub: JSON.stringify(["identity", "agent-3499"]) });
    expect(last?.value?.row.name).toBe("Agent 3499");
  } finally { session.close(); }
});

it("settles the connection ring on HTTPS reconnect and starts a new snapshot on reload", async () => {
  const { httpsDeviceSnapshot } = await import("./fixtures/httpsDeviceSnapshot.js");
  const { greetLiveBridge } = await import("../src/connection.js");
  const sync = await import("../src/core/cacheSync.js");
  vi.stubGlobal("navigator", { locks: { request: (_name, acquire) => Promise.resolve().then(acquire) } });
  // The tab pauses again after a 500-row continuation. Its next request
  // crosses the original lease, and the already-cleared cursor must allow
  // an immediate fresh lease before the connection pass releases the ring.
  let paused = false;
  const fixture = httpsDeviceSnapshot(650, { afterPage({ offset, end, elapse }) {
    if (!paused && end - offset === 500) { paused = true; elapse(300_000); }
  } });
  const transitions = [];
  const stopWatch = sync.subscribeConnectionSync((active) => transitions.push(active));
  const connect = async () => {
    const session = await fixture.openSession();
    await greetLiveBridge(contexts.adoptDeviceSession(session), { suppressFailure: false });
  };
  const settled = () => vi.waitFor(() => expect(transitions).toEqual([false, true, false]), { timeout: 10000 });
  try {
    await connect();
    sync.startCacheSync();
    await settled();
    fixture.advanceThreeDays();
    transitions.splice(1);
    await connect();
    await settled();
    expect(fixture.counts).toMatchObject({ pages: 7, returned: 1251, overBudget: 1, expiries: 1 });
    // A new lock/engine owner starts without the old in-memory continuity
    // proof. Keep IndexedDB populated as it would be after a page reload.
    sync.stopCacheSync();
    transitions.splice(1);
    sync.startCacheSync();
    await settled();
    expect(fixture.counts).toMatchObject({ pages: 10, returned: 1901, overBudget: 1, expiries: 1 });
    const held = await cache.readCached({ ...address("sync-cursor"), sub: JSON.stringify(scope) });
    expect(held?.value?.appliedHead).toBe("3601");
  } finally {
    stopWatch(); sync.stopCacheSync(); contexts.contextFor(deviceId)?.session?.close();
  }
}, 30000);
