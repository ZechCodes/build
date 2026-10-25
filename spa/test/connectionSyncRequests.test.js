// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { called, passesSettled } from "./waits.js";

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: [] } }),
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: () => ({ dispose() {} }),
}));
let context, deviceChanged;
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context, liveContexts: () => context.connected ? [context] : [],
  onDeviceStateChanged: (callback) => { deviceChanged = callback; return () => {}; },
}));
let sync, listener;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  context = { deviceId: "dev-1", session: {}, greeted: Promise.resolve(), connected: true };
  context.active = () => context.connected;
  context.cacheScope = { deviceId: "dev-1", active: context.active };
  context.rpc = vi.fn(async (method) => ({
    "board.list": { items: [] }, "project.list": { projects: [] }, "workspace.list": { workspaces: [] },
  })[method] || {});
  sync = await import("../src/core/cacheSync.js");
  listener = vi.fn(); sync.subscribeConnectionSync(listener);
});
afterEach(() => { sync.stopCacheSync(); vi.restoreAllMocks(); });
const values = () => listener.mock.calls.map(([active]) => active);
function holdBoard() {
  let release;
  const promise = new Promise((resolve) => { release = () => resolve({ items: [] }); });
  context.rpc.mockImplementationOnce(() => promise);
  return release;
}
const boardStarted = (count = 1) => called(context.rpc,
  () => context.rpc.mock.calls.filter(([method]) => method === "board.list").length >= count);
async function settle() {
  await passesSettled(sync, "dev-1");
  await vi.waitFor(() => expect(values().at(-1)).toBe(false));
}

it.each(["ordinary", "restored"])("rejects a %s request without the sync lock", async (kind) => {
  const result = kind === "ordinary" ? sync.syncDevice("dev-1")
    : sync.syncRestoredDevice("dev-1", context.session);
  expect(await result).toBe(false);
  expect(context.rpc).not.toHaveBeenCalled();
  expect(values()).toEqual([false]);
});

it("rejects a restored request for a superseded session without disturbing an active pass", async () => {
  const release = holdBoard();
  sync.startCacheSync(); await boardStarted();
  expect(await sync.syncRestoredDevice("dev-1", {})).toBe(false);
  expect(values()).toEqual([false, true]);
  release(); await settle();
  expect(values()).toEqual([false, true, false]);
});

it.each(["ordinary", "restored"])("abandons a waiting %s request when sync stops", async (kind) => {
  const release = holdBoard();
  sync.startCacheSync(); await boardStarted();
  const previous = sync.passInFlight("dev-1");
  context.session = {};
  const waiting = kind === "ordinary" ? sync.syncDevice("dev-1")
    : sync.syncRestoredDevice("dev-1", context.session);
  sync.stopCacheSync(); release();
  expect(await waiting).toBe(false);
  await previous;
  expect(values()).toEqual([false, true, false]);
});

it("shares the replacement pass when another superseded-session waiter starts it first", async () => {
  const release = holdBoard();
  sync.startCacheSync(); await boardStarted();
  context.session = {}; deviceChanged();
  const second = sync.syncDevice("dev-1");
  release();
  expect(await second).toBe(false);
  await settle();
  expect(context.rpc.mock.calls.filter(([method]) => method === "board.list")).toHaveLength(2);
  expect(values()).toEqual([false, true, false]);
});

it.each(["restart", "reconnect"])("keeps the new request active after an old waiter settles across %s", async (operation) => {
  const releaseOld = holdBoard();
  sync.startCacheSync(); await boardStarted();
  const previous = sync.passInFlight("dev-1");
  const waiting = sync.syncRestoredDevice("dev-1", context.session);
  if (operation === "restart") sync.stopCacheSync();
  else { context.connected = false; deviceChanged(); }
  context.session = {};
  const releaseNew = holdBoard();
  if (operation === "restart") sync.startCacheSync();
  else { context.connected = true; deviceChanged(); }
  // A reconnect waits for the retired pass; a restart immediately gets its own pass.
  if (operation === "restart") await boardStarted(2);
  releaseOld(); await previous; expect(await waiting).toBe(false);
  await boardStarted(2);
  expect(values().at(-1)).toBe(true);
  releaseNew(); await settle();
  expect(values()).toEqual([false, true, false, true, false]);
});

it("does not let a restored waiter resume after a restart on the same session", async () => {
  const release = holdBoard();
  sync.startCacheSync(); await boardStarted();
  const previous = sync.passInFlight("dev-1");
  const waiting = sync.syncRestoredDevice("dev-1", context.session);
  sync.stopCacheSync(); sync.startCacheSync();
  await boardStarted(2);
  const callsBeforeOldAnswer = context.rpc.mock.calls.filter(([method]) => method === "board.list").length;
  release(); await previous;
  expect(await waiting).toBe(false);
  await passesSettled(sync, "dev-1");
  expect(context.rpc.mock.calls.filter(([method]) => method === "board.list")).toHaveLength(callsBeforeOldAnswer);
  expect(values()).toEqual([false, true, false, true, false]);
});
