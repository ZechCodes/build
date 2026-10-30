import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let cache, tracker, summaries, actions, order;
const scope = { deviceId: "watch-lineage-900", projectId: "proj-1" }, id = "task-1";
const freshFloor = "te-01M64A9D058M2B7EMP3N30G6YM";
const task = (extra = {}) => ({ id, project_id: scope.projectId, number: 1, title: "Task", body: "Keep body", state: "open",
  status: "ready", watched: true, read_through: null, dismissed_through: null, watch_started_after: "opaque-zzzz",
  unread_count: 0, unread_mentions: 0, unread_agent_comments: 0, session_started_ms: 11, last_activity_ms: 12,
  updated_at: "2026-10-29T12:00:00Z", ...extra });
const value = async (address) => (await cache.readCached(address))?.value;
const lists = () => [tracker.tasksAddress(scope.deviceId, scope.projectId),
  tracker.tasksQueryAddress(scope.deviceId, scope.projectId, { state: "open" }),
  tracker.tasksPageAddress(scope.deviceId, scope.projectId, { limit: 100 })];
async function land(incoming, basis = "current") {
  const read = await order.nextTaskRead(), ticket = await summaries.beginSummaryQuery(scope, {}, read);
  await summaries.landSummaryPage(ticket, { summary: true, tasks: [incoming], basis }, { read });
  return { read, ticket };
}
async function seed(held = task()) {
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, id, { task: held, timeline: [{ id: "history" }] });
  for (const address of lists()) await cache.writeCached(address, tracker.tasksRecord([held], []));
  await land(held, "prior");
}
const replicas = async () => [(await tracker.readTaskRecord(scope.deviceId, scope.projectId, id)).task,
  ...await Promise.all(lists().map(async (address) => (await value(address)).tasks[0])),
  (await value(summaries.summaryAddress(scope, id))).task];
const publish = async (incoming, issuedRead, accept = () => true) => actions.cacheTaskWaitingOnResponse(scope.deviceId,
  scope.projectId, { task: incoming }, { issuedRead, accept });
const fresh = (extra = {}) => task({ watch_started_after: freshFloor, unread_count: 2, unread_mentions: 1,
  unread_agent_comments: 2, session_started_ms: 21, last_activity_ms: 22, updated_at: "2026-10-29T12:01:00Z", ...extra });

beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js"); tracker = await import("../src/core/trackerCache.js");
  summaries = await import("../src/core/taskSummaryCache.js"); actions = await import("../src/core/taskWaitingOnCache.js");
  order = await import("../src/core/taskReadOrder.js");
});

it("replaces a missed prior watch lineage in every action replica and recovers a fresh-basis query", async () => {
  await seed(); const issued = await order.nextTaskRead(); await publish(fresh(), issued);
  for (const row of await replicas()) expect(row).toMatchObject({ watch_started_after: freshFloor, unread_count: 2,
    unread_mentions: 1, unread_agent_comments: 2, session_started_ms: 21, last_activity_ms: 22 });
  await land(fresh(), "fresh-basis");
  expect(await summaries.readSummaryQuery(scope, {})).toMatchObject({ stale: false, basis: "fresh-basis" });
});

it("recovers an unobserved rewatch from a fresh ordinary summary without an action acknowledgment", async () => {
  await seed(); await land(fresh(), "fresh-basis");
  expect((await value(summaries.summaryAddress(scope, id))).task).toMatchObject({ watch_started_after: freshFloor, unread_count: 2 });
  expect(await summaries.readSummaryQuery(scope, {})).toMatchObject({ stale: false, basis: "fresh-basis" });
});

it("accepts a newer-read current watch lineage while preserving future-dated ordinary content", async () => {
  await seed(task({ title: "Future body", updated_at: "2027-10-29T12:00:00Z" }));
  await land(fresh(), "fresh-basis");
  expect((await value(summaries.summaryAddress(scope, id))).task).toMatchObject({ title: "Future body", watch_started_after: freshFloor, unread_count: 2 });
  expect(await summaries.readSummaryQuery(scope, {})).toMatchObject({ stale: false, basis: "fresh-basis" });
});

it.each([null, ""])("preserves exact current optional watch floor %j and retires missing old-session counters", async (floor) => {
  await seed(); const incoming = fresh({ watch_started_after: floor });
  for (const key of ["unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"]) delete incoming[key];
  await publish(incoming, await order.nextTaskRead());
  for (const row of await replicas()) {
    expect(row.watch_started_after ?? null).toBe(floor);
    expect(row.unread_count).toBe(2);
    for (const key of ["unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"]) expect(row).not.toHaveProperty(key);
  }
});

it("keeps read and dismissal monotonic while refusing counters paired with an overtaken read floor in a new lineage", async () => {
  await seed(task({ read_through: "tc-20", dismissed_through: "te-30" }));
  await publish(fresh({ read_through: "tc-10", dismissed_through: "te-10" }), await order.nextTaskRead());
  for (const row of await replicas()) {
    expect(row).toMatchObject({ read_through: "tc-20", dismissed_through: "te-30", watch_started_after: freshFloor });
    for (const key of ["unread_count", "unread_mentions", "unread_agent_comments"]) expect(row).not.toHaveProperty(key);
  }
  expect((await value(summaries.summaryAddress(scope, id))).needs_unread_refresh).toBe(true);
});

it("rejects an older issued prior-lineage action after a newer action commits", async () => {
  await seed(); const older = await order.nextTaskRead(), newer = await order.nextTaskRead();
  await publish(fresh(), newer); expect(await publish(task(), older)).toBe(false);
  for (const row of await replicas()) expect(row).toMatchObject({ watch_started_after: freshFloor, unread_count: 2 });
});

it("keeps a newer summary lineage when a previously issued action replies later", async () => {
  await seed(); const issued = await order.nextTaskRead(); await land(fresh(), "fresh-basis");
  await publish(task(), issued);
  expect((await value(summaries.summaryAddress(scope, id))).task).toMatchObject({ watch_started_after: freshFloor, unread_count: 2 });
});

it("retains the current lineage against stale prior-lineage summary pages", async () => {
  await seed(); const read = await order.nextTaskRead(), ticket = await summaries.beginSummaryQuery(scope, { states: ["open"] }, read);
  await land(fresh(), "fresh-basis");
  await summaries.landSummaryPage(ticket, { summary: true, tasks: [task()], basis: "prior" }, { read });
  expect((await value(summaries.summaryAddress(scope, id))).task).toMatchObject({ watch_started_after: freshFloor, unread_count: 2 });
});

it("does not reset a watch lineage after captured session authority is replaced during publication", async () => {
  await seed(); const before = await replicas(), capturedSession = {}; let session = capturedSession;
  const original = IDBObjectStore.prototype.get;
  const spy = vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
    if (String(key).includes("|tracker-summary|")) session = {};
    return original.call(this, key);
  });
  expect(await publish(fresh(), await order.nextTaskRead(), () => session === capturedSession)).toBe(false);
  spy.mockRestore(); expect(session).not.toBe(capturedSession);
  expect(await replicas()).toEqual(before);
});
