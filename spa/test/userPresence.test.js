/** @vitest-environment jsdom */
// The user arriving at this client is told to each bridge it is connected to,
// and nothing automatic is an arrival.

import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

let carries = true;
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ issues: { doneSinceLeft: carries } }),
}));

const contexts = [];
const stateListeners = new Set();
vi.mock("../src/core/deviceContexts.js", () => ({
  liveContexts: () => [...contexts],
  onDeviceStateChanged: (fn) => {
    stateListeners.add(fn);
    return () => stateListeners.delete(fn);
  },
}));

const { ARRIVAL_FRESH_MS, PRESENCE_EVERY_MS, resetUserPresence, startUserPresence } = await import("../src/core/userPresence.js");
const { readUserSession } = await import("../src/core/userSessionCache.js");

const SESSION = {
  session_started_ms: 50, last_activity_ms: 50, previous_session_ended_ms: 10, gap_ms: 21_600_000, now_ms: 50,
};
let rpc;
const connect = (deviceId = "dev-1") => {
  contexts.push({ deviceId, rpc, greeted: Promise.resolve() });
  stateListeners.forEach((fn) => fn());
};
const sent = () => rpc.mock.calls.filter(([method]) => method === "user.present").length;
let focused;

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  carries = true;
  contexts.length = 0;
  rpc = vi.fn(async () => ({ user_session: SESSION }));
  focused = vi.spyOn(document, "hasFocus").mockReturnValue(true);
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(Date.parse("2026-09-22T09:00:00Z"));
});

afterEach(() => {
  resetUserPresence();
  focused.mockRestore();
  vi.useRealTimers();
});

it("tells a connected bridge when the app opens in a focused window, and caches what it answers", async () => {
  connect();
  startUserPresence();
  await vi.waitFor(() => expect(sent()).toBe(1));
  await vi.waitFor(async () => expect((await readUserSession("dev-1"))?.session_started_ms).toBe(50));
});

it("says nothing from an unfocused window until the user comes back to it", async () => {
  focused.mockReturnValue(false);
  connect();
  startUserPresence();
  document.dispatchEvent(new Event("pointerdown"));
  await Promise.resolve();
  expect(sent()).toBe(0);

  focused.mockReturnValue(true);
  window.dispatchEvent(new Event("focus"));
  await vi.waitFor(() => expect(sent()).toBe(1));
});

it("tells a bridge that connects soon after the arrival, and not one that reconnects hours later", async () => {
  startUserPresence();
  connect("dev-1");
  await vi.waitFor(() => expect(sent()).toBe(1));

  // Overnight, nobody at the focused window: a reconnect is not an arrival.
  vi.setSystemTime(Date.now() + ARRIVAL_FRESH_MS + 1);
  connect("dev-2");
  await Promise.resolve();
  expect(sent()).toBe(1);
});

it("tells each bridge at most once a minute however busy the pointer is", async () => {
  connect();
  startUserPresence();
  await vi.waitFor(() => expect(sent()).toBe(1));
  document.dispatchEvent(new Event("pointerdown"));
  document.dispatchEvent(new Event("keydown"));
  window.dispatchEvent(new Event("hashchange"));
  await Promise.resolve();
  expect(sent()).toBe(1);

  vi.setSystemTime(Date.now() + PRESENCE_EVERY_MS);
  document.dispatchEvent(new Event("pointerdown"));
  await vi.waitFor(() => expect(sent()).toBe(2));
});

it("asks nothing of a bridge that does not record arrivals", async () => {
  carries = false;
  connect();
  startUserPresence();
  await Promise.resolve();
  await Promise.resolve();
  expect(sent()).toBe(0);
});
