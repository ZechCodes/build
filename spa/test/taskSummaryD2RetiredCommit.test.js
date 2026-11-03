import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { task as wireTask } from "./trackerWireFixture.js";

// Actual commit/no-op/abort witnesses for the local retirement hint. Commit
// notification is scheduling only and never task/query/watch authority.
let cache, summaries, content, tracker, order, projection, keys, stops;
const scope = { deviceId: "d2-retired-commit-891", projectId: "proj-1" };
const basis = "opaque-commit-control", cursor = "opaque-before-retirement";
const queryAddress = () => summaries.summaryQueryAddress(scope, {});
const rowAddress = () => summaries.summaryAddress(scope, "task-2");
const full = (extra = {}) => wireTask({ id: "task-2", project_id: scope.projectId, number: 2,
  title: "Task 2", body: "Body 2", status: "ready", watched: true, attachments: [],
  read_through: "tc-10", watch_started_after: "te-01", dismissed_through: "te-05",
  unread_count: 2, unread_mentions: 1, unread_agent_comments: 1,
  review_summary: { task_id: "task-2", version: 1, status: "open", mode: "pull_request" },
  updated_at: "2026-10-29T12:00:00Z", ...extra });
const row = (extra = {}) => ({ ...projection.fullTaskSummary(full(extra)), summary_truncated: false });
const page = (tasks, identities = {}) => ({ project_id: scope.projectId, summary: true, basis,
  tasks, identities, next_cursor: "opaque-must-not-consume" });
const factsPage = () => page([row({ read_through: "tc-20", unread_count: 1,
  unread_mentions: 0, unread_agent_comments: 1 })], { new: { name: "New" } });

beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange; stops = [];
  cache = await import("../src/core/localCache.js"); summaries = await import("../src/core/taskSummaryCache.js");
  content = await import("../src/core/taskContentAuthority.js"); tracker = await import("../src/core/trackerCache.js");
  order = await import("../src/core/taskReadOrder.js"); projection = await import("../src/core/taskSummaryProjection.js");
  keys = await import("../src/core/idbRecords.js");
});
afterEach(() => { stops.forEach((stop) => stop()); vi.restoreAllMocks(); });

async function retiredTicket() {
  const first = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const ticket = await summaries.beginSummaryQuery(scope, {}, first.readOrder, { contentRead: first });
  expect(await summaries.landSummaryPage(ticket, { ...page([row()]), next_cursor: cursor }, { read: first.readOrder })).toBe(true);
  expect(await summaries.readSummaryQuery(scope, {})).toMatchObject({ ids: ["task-2"], basis,
    next_cursor: cursor, stale: false, snapshot_continuation_eligible: true });
  const next = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const before = await cache.readCached(order.taskContentScopeAddress(scope));
  vi.resetModules(); const other = await import("../src/core/trackerCache.js");
  expect(await other.writeTaskRecord(scope.deviceId, scope.projectId, "task-2", {
    task: full({ title: "Protected local edit", body: "Protected local body" }), timeline: ["Local history"],
  })).toBe(true);
  expect((await cache.readCached(order.taskContentScopeAddress(scope))).value.retirementEpoch).not.toBe(before.value.retirementEpoch);
  return { ...ticket, cursor, contentRead: next };
}

function watchTransactions(events) {
  const transaction = IDBDatabase.prototype.transaction;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function(...args) {
    const tx = transaction.apply(this, args);
    if (args[1] === "readwrite") {
      tx.addEventListener("complete", () => events.push("complete"));
      tx.addEventListener("abort", () => events.push("abort"));
    }
    return tx;
  });
}

function publications(events) {
  stops.push(cache.subscribeCache({ deviceId: scope.deviceId, entityId: scope.projectId }, () => events.push("announce")));
}

function lateFault(name, onFault = () => {}) {
  const put = IDBObjectStore.prototype.put, state = { failures: 0, queuedRows: 0 };
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function(record, key) {
    if (record?.value?.identity?.name === "New" && state.failures === 0) {
      state.failures += 1; onFault(); throw new DOMException("D2 late transaction fault", name);
    }
    const request = put.call(this, record, key);
    if (record?.value?.task?.id === "task-2") state.queuedRows += 1;
    return request;
  });
  return state;
}

function protectedAddresses() {
  return [queryAddress(), rowAddress(), tracker.taskAddress(scope.deviceId, scope.projectId, "task-2"),
    { deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-summary-identity", sub: "new" },
    order.taskContentScopeAddress(scope)];
}

it("D2-C01 verifies committed no-op notification while preserving the legacy false merge result", async () => {
  const address = rowAddress(); await cache.writeCached(address, { sentinel: "unchanged" });
  const before = await cache.readCached(address);
  expect(await cache.mergeCachedRecordsTogether([address], () => [null])).toBe(false);
  const events = [], notified = vi.fn(() => events.push("hint")); watchTransactions(events); publications(events);
  expect(await cache.mergeCachedRecordsTogether([address], () => [null], { onCommit: notified })).toBe(false);
  expect(events).toEqual(["complete", "hint"]); expect(notified).toHaveBeenCalledTimes(1);
  expect(await cache.readCached(address)).toEqual(before);
});

it("D2-C02 isolates a throwing reentrant commit observer from boolean results and announcements", async () => {
  const address = rowAddress(), events = []; let reentrant;
  await cache.writeCached(address, { count: 0 }); publications(events);
  const onCommit = vi.fn(() => {
    events.push("hint");
    reentrant = cache.mergeCachedRecordsTogether([address], () => [{ count: 2 }]);
    throw new Error("Local observer failure");
  });
  expect(await cache.mergeCachedRecordsTogether([address], () => [{ count: 1 }], { onCommit })).toBe(true);
  expect(onCommit).toHaveBeenCalledTimes(1); expect(events.slice(0, 2)).toEqual(["hint", "announce"]);
  expect(await reentrant).toBe(true);
  expect(events).toEqual(["hint", "announce", "announce"]);
  expect((await cache.readCached(address)).value).toEqual({ count: 2 });
});

it("D2-C03 emits one retired empty-page hint only after the actual no-write transaction commits", async () => {
  const ticket = await retiredTicket(), addresses = protectedAddresses(), before = await cache.readCachedMany(addresses);
  const events = [], hint = vi.fn(() => events.push("hint")); watchTransactions(events); publications(events);
  expect(await summaries.landSummaryPage(ticket, page([]), { read: ticket.contentRead.readOrder, onQueryRetired: hint })).toBe(false);
  expect(events).toEqual(["complete", "hint"]); expect(hint).toHaveBeenCalledTimes(1);
  expect(await cache.readCachedMany(addresses)).toEqual(before);
});

it("D2-C04 emits no retired hint or announcement after a real late quota abort", async () => {
  const ticket = await retiredTicket(), addresses = protectedAddresses(), before = await cache.readCachedMany(addresses);
  const events = [], hint = vi.fn(() => events.push("hint")); watchTransactions(events); publications(events);
  const fault = lateFault("QuotaExceededError");
  expect(await summaries.landSummaryPage(ticket, factsPage(), { read: ticket.contentRead.readOrder, onQueryRetired: hint })).toBe(false);
  expect(fault).toEqual({ failures: 1, queuedRows: 1 });
  expect(await cache.readCachedMany(addresses)).toEqual(before);
  expect(events.filter((event) => event !== "hint")).toEqual(["abort"]);
  expect(hint).not.toHaveBeenCalled(); expect(events).toEqual(["abort"]);
});

it("D2-C05 emits one final committed retry hint before subscribers can demand recovery", async () => {
  const ticket = await retiredTicket(), queryBefore = await cache.readCached(queryAddress());
  const events = [], hint = vi.fn(() => events.push("hint")); watchTransactions(events); publications(events);
  const fault = lateFault("InvalidStateError");
  expect(await summaries.landSummaryPage(ticket, factsPage(), { read: ticket.contentRead.readOrder, onQueryRetired: hint })).toBe(false);
  expect(fault).toEqual({ failures: 1, queuedRows: 2 });
  expect(hint).toHaveBeenCalledTimes(1);
  expect(events.slice(0, 3)).toEqual(["abort", "complete", "hint"]);
  expect(events.filter((event) => event === "announce")).toHaveLength(2);
  expect((await cache.readCached(rowAddress())).value.task.read_through).toBe("tc-20");
  expect(await cache.readCached(queryAddress())).toEqual(queryBefore);
});

it("D2-C06 resets a failed attempt retirement flag when replacement session refuses its retry", async () => {
  const ticket = await retiredTicket(), addresses = protectedAddresses(), before = await cache.readCachedMany(addresses);
  let session = {}; const capturedSession = session;
  const events = [], hint = vi.fn(() => events.push("hint")); watchTransactions(events); publications(events);
  const fault = lateFault("InvalidStateError", () => { session = {}; });
  expect(await summaries.landSummaryPage(ticket, factsPage(), { read: ticket.contentRead.readOrder,
    accept: () => session === capturedSession, onQueryRetired: hint })).toBe(false);
  expect(fault).toEqual({ failures: 1, queuedRows: 1 });
  expect(events).toEqual(["abort", "complete"]); expect(hint).not.toHaveBeenCalled();
  expect(await cache.readCachedMany(addresses)).toEqual(before);
});

it("D2-C07 withholds a committed retired hint from an owner superseded after its reducer", async () => {
  const ticket = await retiredTicket(), queryBefore = await cache.readCached(queryAddress());
  const events = [], hint = vi.fn(() => events.push("hint")); watchTransactions(events); publications(events);
  let generation = 1, superseded = 0;
  const get = IDBObjectStore.prototype.get, finalKey = keys.recordKey(order.taskWatchIntentAddress(scope, "task-2"));
  vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function(key) {
    const request = get.call(this, key);
    if (key !== finalKey || this.transaction.mode !== "readwrite" || superseded) return request;
    let callback;
    Object.defineProperty(request, "onsuccess", { configurable: true, get: () => callback,
      set(admit) { callback = (...args) => { admit(...args); generation += 1; superseded += 1; }; } });
    return request;
  });
  expect(await summaries.landSummaryPage(ticket, factsPage(), { read: ticket.contentRead.readOrder,
    accept: () => generation === 1, onQueryRetired: hint })).toBe(false);
  expect(superseded).toBe(1); expect(events[0]).toBe("complete");
  expect((await cache.readCached(rowAddress())).value.task.read_through).toBe("tc-20");
  expect(await cache.readCached(queryAddress())).toEqual(queryBefore);
  expect(hint).not.toHaveBeenCalled();
});
