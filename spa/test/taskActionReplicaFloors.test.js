import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let cache, tracker, summaries, actions, order;
const scope = { deviceId: "action-replica-900", projectId: "proj-1" }, taskId = "task-1";
const task = (extra = {}) => ({ id: taskId, project_id: scope.projectId, number: 1, title: "Before", state: "open",
  status: "ready", watched: true, read_through: "tc-20", watch_started_after: null, dismissed_through: "te-20",
  unread_count: 0, unread_mentions: 0, unread_agent_comments: 0, updated_at: "2026-10-29T12:00:00Z", ...extra });
const value = async (address) => (await cache.readCached(address))?.value;
const listAddresses = () => [tracker.tasksAddress(scope.deviceId, scope.projectId),
  tracker.tasksQueryAddress(scope.deviceId, scope.projectId, { state: "open" }),
  tracker.tasksPageAddress(scope.deviceId, scope.projectId, { limit: 100 })];
async function seed(held) {
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId, { task: { ...held, body: "Full held body" }, timeline: [{ id: "history" }] });
  await Promise.all(listAddresses().map((address) => cache.writeCached(address, tracker.tasksRecord([held], []))));
  const read = await order.nextTaskRead(), ticket = await summaries.beginSummaryQuery(scope, {}, read);
  await summaries.landSummaryPage(ticket, { summary: true, tasks: [held], basis: "before" }, { read });
}
async function replicas() {
  return [(await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId)).task,
    ...await Promise.all(listAddresses().map(async (address) => (await value(address)).tasks[0])),
    (await value(summaries.summaryAddress(scope, taskId))).task];
}
const publish = async (incoming, options = {}) => actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId,
  { task: { ...incoming, body: "Full incoming body", attachments: [] } }, { issuedRead: await order.nextTaskRead(), ...options });
const allAddresses = () => [tracker.taskAddress(scope.deviceId, scope.projectId, taskId), ...listAddresses(),
  ...listAddresses().map(order.readsAddress), order.readsAddress(tracker.taskAddress(scope.deviceId, scope.projectId, taskId)),
  summaries.summaryAddress(scope, taskId), { deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-summary-invalidation", sub: "" }];

beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js"); tracker = await import("../src/core/trackerCache.js");
  summaries = await import("../src/core/taskSummaryCache.js"); actions = await import("../src/core/taskWaitingOnCache.js");
  order = await import("../src/core/taskReadOrder.js");
});

it.each([
  ["tc-20", "te-10", "tc-20", 0],
  ["tc-20\0", "te-10\0", "tc-20\0", 0],
  ["", null, "", 0],
  ["tc-\u{10000}", "te-\ue000", "tc-\u{10000}", 0],
  ["tc-\ue000", "te-\u{10000}", "te-\u{10000}", 9],
])("pairs canonical full-action read floor %j/%j with its counters in every replica", async (heldRead, incomingRead, expectedRead, count) => {
  await seed(task({ read_through: heldRead, dismissed_through: heldRead }));
  await publish(task({ title: "Fresh content", read_through: incomingRead, dismissed_through: incomingRead,
    unread_count: 9, unread_mentions: 9, unread_agent_comments: 9, updated_at: "2026-10-29T12:01:00Z" }));
  for (const replica of await replicas()) expect(replica).toMatchObject({ title: "Fresh content", read_through: expectedRead,
    dismissed_through: expectedRead, unread_count: count, unread_mentions: count, unread_agent_comments: count });
});

it.each([
  [{ read_through: "tc-20", watch_started_after: "te-05", unread_count: 2 },
    { read_through: "tc-10", watch_started_after: "te-30", unread_count: 0, unread_mentions: 3, unread_agent_comments: 4 },
    { read_through: "tc-20", watch_started_after: "te-30", unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 }],
  [{ read_through: "tc-10", watch_started_after: "te-30", unread_count: 0, unread_mentions: 2, unread_agent_comments: 3 },
    { read_through: "tc-20", watch_started_after: "te-05", unread_count: 4 },
    { read_through: "tc-20", watch_started_after: "te-30", unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 }],
])("keeps read-only attention counters separate from the combined read/watch count family %#", async (held, incoming, expected) => {
  await seed(task(held));
  await publish(task({ ...incoming, updated_at: "2026-10-29T12:01:00Z" }));
  for (const replica of await replicas()) expect(replica).toMatchObject(expected);
  expect((await value(summaries.summaryAddress(scope, taskId))).needs_unread_refresh).toBe(true);
});

it("does not attach weaker-floor counters when the retained stronger floor has unknown counters", async () => {
  const held = task();
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete held[field];
  await seed(held);
  await publish(task({ read_through: "tc-10", unread_count: 9, unread_mentions: 9, unread_agent_comments: 9 }));
  for (const replica of await replicas()) {
    expect(replica.read_through).toBe("tc-20");
    for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) expect(replica).not.toHaveProperty(field);
  }
});

it("clears the watch lineage in every replica on a proved authoritative unwatch", async () => {
  await seed(task({ watch_started_after: "te-30", session_started_ms: 1, last_activity_ms: 2 }));
  await publish(task({ watched: false, read_through: "tc-10", watch_started_after: "", unread_count: 9 }));
  for (const replica of await replicas()) {
    expect(replica).toMatchObject({ watched: false, read_through: "tc-20" });
    for (const field of ["watch_started_after", "unread_count", "unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"]) expect(replica).not.toHaveProperty(field);
  }
});

it("refuses an unissued compatibility reply after a proved action instead of blessing its arrival order", async () => {
  await seed(task()); await publish(task({ watched: false }));
  const before = await Promise.all(allAddresses().map(value));
  expect(await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: task({ watched: true }) })).toBe(false);
  expect(await Promise.all(allAddresses().map(value))).toEqual(before);
});

it("rolls every legacy and normalized replica and note back if the joint marker publication hits quota", async () => {
  await seed(task()); const before = await Promise.all(allAddresses().map(value));
  const original = IDBObjectStore.prototype.put;
  const spy = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (String(key).includes("|tracker-summary|") && record?.value?.task?.title === "Quota") throw new DOMException("Quota", "QuotaExceededError");
    return original.call(this, record, key);
  });
  expect(await publish(task({ title: "Quota", read_through: "tc-10", unread_count: 9 }))).toBe(false);
  spy.mockRestore(); expect(await Promise.all(allAddresses().map(value))).toEqual(before);
});
