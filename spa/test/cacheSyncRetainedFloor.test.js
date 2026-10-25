// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { called, passesSettled } from "./waits.js";

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
let watchers, context;
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "thread"] } }),
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: (registration) => { watchers.push(registration); return { dispose() {} }; },
}));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context, liveContexts: () => [context],
  onDeviceStateChanged: () => () => {},
}));
const target = { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "chat" };
const message = (sequence) => ({ type: "message", data: { id: `m${sequence}`, sequence, body: `Message ${sequence}` } });
const event = (floor, { since = 10, last = 11, items = [message(11)], threadId = "current" } = {}) => [{
  entity_id: target.entityId,
  thread: [{ agent_id: "agent-1", conversation_id: target.sub, thread_id: threadId, thread_generation_revision: 2,
    history_start_sequence: floor, since_sequence: since, last_sequence: last, items }],
}];
let cache, sync;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  watchers = [];
  context = { deviceId: target.deviceId, session: {}, greeted: Promise.resolve(), active: () => true,
    cacheScope: { deviceId: target.deviceId, active: () => true },
    rpc: vi.fn(async (method) => ({
      "board.list": { items: [] }, "project.list": { projects: [] }, "workspace.list": { workspaces: [] },
    })[method] || {}) };
  cache = await import("../src/core/localCache.js"); sync = await import("../src/core/cacheSync.js");
  sync.startCacheSync(); await passesSettled(sync, target.deviceId);
  await cache.writeCached(target, { thread_id: "current", thread_generation_revision: 2,
    retired_thread_ids: ["retired"], deliveredSequence: 10, historyStartSequence: 5, items: [5, 8, 10].map(message), activityDigests: [] });
});
afterEach(async () => { sync.stopCacheSync(); vi.restoreAllMocks(); await passesSettled(sync, target.deviceId); });
const push = (items) => watchers.find((watcher) => watcher.kinds.includes("thread")).onChanges(items);

it.each([5, 8])("publishes a carried tip and its retained floor %s in one commit", async (floor) => {
  let commits = 0;
  const stop = cache.subscribeCache(target, () => { commits += 1; });
  try {
    await push(event(floor));
    expect(commits).toBe(1);
    const held = (await cache.readCached(target)).value;
    expect(held.historyStartSequence).toBe(floor);
    expect(held.deliveredSequence).toBe(11);
    expect(held.items.map((item) => item.data.sequence)).toEqual([5, 8, 10, 11].filter((sequence) => sequence >= floor));
  } finally { stop(); }
});

it.each([[], [message(16)]])("publishes an advancing floor before a held gap repair without claiming its coverage (%j)", async (items) => {
  let release;
  const repair = new Promise((resolve) => { release = resolve; });
  const original = context.rpc.getMockImplementation();
  context.rpc.mockImplementation((method, params) => method === "thread.page" ? repair : original(method, params));
  const pending = push(event(8, { since: 15, last: 16, items }));
  try {
    await called(context.rpc, () => context.rpc.mock.calls.some(([method]) => method === "thread.page"));
    const held = (await cache.readCached(target)).value;
    expect(held.historyStartSequence).toBe(8);
    expect(held.deliveredSequence).toBe(10);
    expect(held.items.map((item) => item.data.sequence)).toEqual([8, 10]);
  } finally {
    release({ thread_id: "current", thread_last_sequence: 10, items: [], has_more: false });
    await pending;
  }
});

it("does not publish a stale generation's carried tip or retention floor", async () => {
  let commits = 0;
  const stop = cache.subscribeCache(target, () => { commits += 1; });
  try {
    await push(event(12, { threadId: "retired" }));
    expect(commits).toBe(0);
    expect((await cache.readCached(target)).value).toMatchObject({ thread_id: "current", deliveredSequence: 10, historyStartSequence: 5 });
  } finally { stop(); }
});

it("clears repair debt when the carried floor retires every previously held item", async () => {
  await cache.writeCached(target, { thread_id: "current", thread_generation_revision: 2,
    deliveredSequence: 10, historyStartSequence: 5, repairThrough: 10,
    items: [5, 10].map(message), activityDigests: [] });
  let commits = 0;
  const stop = cache.subscribeCache(target, () => { commits += 1; });
  try {
    await push(event(11));
    const held = (await cache.readCached(target)).value;
    expect(held.repairThrough).toBeUndefined();
    await push(event(11, { since: 11, last: 11, items: [] }));
    expect(context.rpc.mock.calls.filter(([method]) => method === "thread.page")).toHaveLength(0);
    expect(held.items.map((item) => item.data.sequence)).toEqual([11]);
    expect(held.deliveredSequence).toBe(11);
    expect(held.historyStartSequence).toBe(11);
    expect(commits).toBe(1);
  } finally { stop(); }
});

it("retains repair debt for an older held item surviving a carried floor", async () => {
  await cache.writeCached(target, { thread_id: "current", thread_generation_revision: 2,
    deliveredSequence: 10, historyStartSequence: 5, repairThrough: 10,
    items: [5, 10].map(message), activityDigests: [] });
  await push(event(8));
  const held = (await cache.readCached(target)).value;
  expect(held.repairThrough).toBe(10);
  expect(held.items.map((item) => item.data.sequence)).toEqual([10, 11]);
  expect(held.deliveredSequence).toBe(11);
});
