// @vitest-environment jsdom
// #324: the feed model's cached tracker rows render before any greeting.
// The mounted rail follows tasks.list/get instead (watchedTaskInboxDom.test.js);
// this covers inboxEntries and its row renderer, including Recent classification.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const DEVICE = "cached-inbox-device";
const ADDRESS = { deviceId: DEVICE, entityId: "", kind: "feed" };
const NOW = Date.parse("2026-10-02T00:10:00Z");
const row = (over = {}) => ({
  kind: "tracker_task", task_id: "task-324", number: 324, title: "Cached task",
  deviceId: DEVICE, project_id: "proj-1", projectKey: `${DEVICE}/proj-1`, project: "Build",
  status: "in_review", unread: 2, anchor: "2026-10-02T00:05:00Z",
  last_activity: "2026-10-02T00:05:00Z", last_event: { text: "Choose the fix" },
  ...over,
});

let cache, contexts, connection, inbox, changes;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="active"></div><div id="recent"></div>';
  cache = await import("../src/core/localCache.js");
  contexts = await import("../src/core/deviceContexts.js");
  connection = await import("../src/connection.js");
  inbox = await import("../src/core/inbox.js");
  changes = await import("../src/core/changeEvents.js");
  await cache.writeCached(ADDRESS, { items: [row(), row({ task_id: "task-cleared", number: 325,
    title: "Cleared task", unread: 0, done_until_next: true })] });
});

afterEach(() => {
  changes?.resetChangeEvents();
  contexts?.resetDeviceContexts();
});

async function paintCachedRows() {
  const { value } = await cache.readCached(ADDRESS);
  const { entries, recent } = inbox.inboxEntries({ ...value, nowMs: NOW });
  document.querySelector("#active").innerHTML = entries.map((entry) => inbox.inboxRowHtml(entry)).join("");
  document.querySelector("#recent").innerHTML = recent.map((entry) => inbox.inboxRowHtml(entry)).join("");
  return document.body.innerHTML;
}

it.each([true, false])("paints cached rows before hello and keeps them when watching is %s", async (watching) => {
  expect(contexts.contextFor(DEVICE)).toBe(null);
  expect(changes.bridgeCapabilities(DEVICE).tasks.watching).toBe(false);
  const cold = await paintCachedRows();
  expect(document.querySelector('#active [data-key="tracker_task:task-324"] .stitle')?.textContent)
    .toBe("#324 Cached task");
  expect(document.querySelector("#active .inbox-facts")?.textContent).toBe("Choose the fix");
  expect(document.querySelector("#active .inbox-actions > .inbox-status-unread")).not.toBeNull();
  expect(document.querySelector("#active .inbox-unread, #active .sdot")).toBeNull();
  expect(document.querySelector('#recent [data-key="tracker_task:task-cleared"] .stitle')?.textContent)
    .toBe("#325 Cleared task");

  const call = vi.fn(async (method) => method === "session.hello"
    ? { api_version: "2.0.0", push_events: true, capabilities: watching ? ["tasks.watching"] : [] } : {});
  const context = contexts.adoptDeviceSession({ deviceId: DEVICE, call,
    close() {}, peer() {}, onCarrier() {}, installAdapter: (selection) => selection.create(call) });
  await connection.greetLiveBridge(context);
  expect(changes.bridgeCapabilities(DEVICE).tasks.watching).toBe(watching);
  expect(await paintCachedRows()).toBe(cold);
  expect(call.mock.calls.some(([method]) => ["board.list", "tasks.list", "tasks.get"].includes(method))).toBe(false);
});
