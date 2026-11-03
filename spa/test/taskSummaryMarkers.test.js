import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { rememberTaskUnreadDisplay } from "./taskUnreadDisplayFixture.js";

let summaries;
const scope = { deviceId: "markers-device", projectId: "proj-1" };
const fields = ["read_through", "watch_started_after", "dismissed_through"];
const row = (mark, count) => ({ id: "task-1", title: "One", watched: true, unread_count: count, unread_mentions: 0, unread_agent_comments: 0,
  ...Object.fromEntries(fields.map((field) => [field, mark])) });
async function land(generation, task) {
  const ticket = await summaries.beginSummaryQuery(scope, {}, generation);
  return summaries.landSummaryPage(ticket, { summary: true, basis: `B${generation}`, tasks: [task] }, { read: generation });
}

beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  summaries = await import("../src/core/taskSummaryCache.js");
});

// Native TaskSummaryView, including its summary-only nested DTOs. None marker
// fields are omitted; explicit null from the former minimal fixture is not a
// native serialized marker. These helpers do not change other marker tests.
const nativeSummary = (extra = {}) => ({
  id: "task-1", project_id: scope.projectId, number: 1, title: "One",
  state: "open", status: "ready", labels: [], priority: "none", assignee: null,
  created_by: { kind: "user" }, created_at: "2026-10-29T00:00:00Z",
  updated_at: "2026-10-29T00:01:00Z", closed_at: null, done_at: null,
  watched: true, unread_mentions: 0, unread_agent_comments: 0, trackers: [], links: { workspace_ids: [], parent_task_id: null },
  attachment_count: 0, summary_truncated: false,
  dependencies: { blocked: false, waiting_on_count: 0, blocks_count: 0 },
  ...extra,
});
const nativePage = (task, basis) => ({
  summary: true, project_id: scope.projectId, basis, tasks: [task], identities: {},
  user_session: { session_started_ms: null, last_activity_ms: null,
    previous_session_ended_ms: null, gap_ms: 1800000, now_ms: 1793232060000 },
});
const nativeFloors = (mark, unread_count = 0) => nativeSummary({ unread_count,
  read_through: mark, watch_started_after: mark, dismissed_through: mark });
async function nativeRequest(filters = {}) {
  const content = await import("../src/core/taskContentAuthority.js");
  const contentRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  expect(contentRead.readOrder).toBeGreaterThan(0);
  const ticket = await summaries.beginSummaryQuery(scope, filters, contentRead.readOrder, { contentRead });
  expect(ticket.contentRead).toBe(contentRead);
  return ticket;
}
async function publishNative(ticket, task) {
  expect(await summaries.landSummaryPage(ticket, nativePage(task, `B${ticket.generation}`),
    { read: ticket.contentRead.readOrder })).toBe(true);
}
async function landNative(task, filters = {}) {
  const ticket = await nativeRequest(filters);
  await publishNative(ticket, task);
  return ticket;
}
async function normalizedNative() {
  const cache = await import("../src/core/localCache.js");
  return (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value;
}
async function expectNativeWatchCertain() {
  const watch = await import("../src/core/taskWatchAuthority.js");
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ uncertain: false, eligible: true });
  expect((await normalizedNative()).watch_uncertain).toBe(false);
}

it.each([
  { label: "empty", mark: "" }, { label: "space", mark: " " },
  { label: "NUL", mark: "\u0000" }, { label: "unprefixed", mark: "withoutprefix" },
  { label: "other-prefix", mark: "other-a-b" }, { label: "astral", mark: "tc-\u{10000}" },
])("keeps exact read/dismiss floor ($label) while an admitted native origin watch replaces its baseline", async ({ mark }) => {
  await landNative(nativeFloors(mark));
  const before = (await summaries.readSummaryQuery(scope, {})).tasks[0];
  const display = await rememberTaskUnreadDisplay(scope, before);
  for (const field of fields) expect(before[field]).toBe(mark);
  const origin = nativeSummary({ unread_count: 9 });
  for (const field of fields) expect(origin).not.toHaveProperty(field);
  await landNative(origin);
  const task = (await summaries.readSummaryQuery(scope, {})).tasks[0];
  expect(task.read_through).toBe(mark);
  expect(task.dismissed_through).toBe(mark);
  expect(task).not.toHaveProperty("watch_started_after");
  expect(task).not.toHaveProperty("unread_count");
  expect(display(task)).toBe(0);
  await expectNativeWatchCertain();
});

it("keeps present empty current watch baseline distinct from native omitted origin baseline", async () => {
  await landNative(nativeFloors("tc-opaque"));
  const origin = nativeSummary({ unread_count: 9 });
  expect(Object.hasOwn(origin, "watch_started_after")).toBe(false);
  // Explicit null is a separate legacy input, not the native emitted DTO.
  expect(Object.hasOwn(row(null, 9), "watch_started_after")).toBe(true);
  expect(row(null, 9).watch_started_after).toBeNull();
  await landNative(origin);
  let held = await normalizedNative();
  expect(held.task).not.toHaveProperty("watch_started_after");
  expect(held.task).toMatchObject({ read_through: "tc-opaque", dismissed_through: "tc-opaque" });
  await expectNativeWatchCertain();
  const empty = nativeSummary({ watch_started_after: "", unread_count: 9 });
  expect(Object.hasOwn(empty, "watch_started_after")).toBe(true);
  await landNative(empty);
  held = await normalizedNative();
  expect(Object.hasOwn(held.task, "watch_started_after")).toBe(true);
  expect(held.task.watch_started_after).toBe("");
  expect(held.task).toMatchObject({ read_through: "tc-opaque", dismissed_through: "tc-opaque" });
  await expectNativeWatchCertain();
});

it("keeps a newer watch baseline against an earlier-issued complete native origin reply", async () => {
  await landNative(nativeFloors("tc-10"));
  const older = await nativeRequest({ states: ["open"] });
  const newer = await landNative(nativeFloors("te-current"));
  expect(older.contentRead.readOrder).toBeLessThan(newer.contentRead.readOrder);
  const before = await normalizedNative();
  await publishNative(older, nativeSummary({ unread_count: 9, read_through: "tc-zz", dismissed_through: "te-zz" }));
  const held = await normalizedNative();
  expect(held.task.watch_started_after).toBe("te-current");
  expect(held.watch_read_order).toBe(before.watch_read_order);
  expect(held.task).toMatchObject({ read_through: "tc-zz", dismissed_through: "te-zz" });
  await expectNativeWatchCertain();
});

it("keeps watch floor and proof when a native origin summary is refused in its transaction", async () => {
  await landNative(nativeFloors("te-held"));
  const ticket = await nativeRequest();
  const cache = await import("../src/core/localCache.js");
  const order = await import("../src/core/taskReadOrder.js");
  const addresses = [summaries.summaryAddress(scope, "task-1"), ticket.address,
    order.taskWatchControlAddress(scope), order.taskWatchIntentAddress(scope, "task-1")];
  const before = await cache.readCachedMany(addresses);
  const accept = vi.fn(() => false);
  expect(await summaries.landSummaryPage(ticket, nativePage(nativeSummary({ unread_count: 9 }), "refused"),
    { read: ticket.contentRead.readOrder, accept })).toBe(false);
  expect(accept).toHaveBeenCalledTimes(1);
  expect(await cache.readCachedMany(addresses)).toEqual(before);
  await expectNativeWatchCertain();
});

it("keeps current watch floor when a compact patch omits the watch header", async () => {
  await landNative(nativeFloors("te-held"));
  const before = await normalizedNative();
  const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
  expect(await summaries.writeSummaryTask(scope, { id: "task-1", title: "Compact ordinary patch" },
    await nextTaskRead())).toBe(true);
  const held = await normalizedNative();
  expect(held.task).toMatchObject({ watched: true, watch_started_after: "te-held",
    read_through: "te-held", dismissed_through: "te-held" });
  expect(held.watch_read_order).toBe(before.watch_read_order);
  expect(held.task.__task_watch_cache).toEqual(before.task.__task_watch_cache);
  await expectNativeWatchCertain();
});

it("admits native explicit-false watch state as an authoritative unwatch", async () => {
  await landNative(nativeFloors("te-held", 3));
  const unwatched = nativeSummary({ watched: false });
  await landNative(unwatched);
  const held = await normalizedNative();
  expect(held.task.watched).toBe(false);
  expect(held.task).not.toHaveProperty("watch_started_after");
  expect(held.task).not.toHaveProperty("unread_count");
  await expectNativeWatchCertain();
});

it("keeps a current floor against a retired admission-epoch origin reply", async () => {
  await landNative(nativeFloors("te-held"));
  const content = await import("../src/core/taskContentAuthority.js");
  const cache = await import("../src/core/localCache.js");
  const order = await import("../src/core/taskReadOrder.js");
  const address = summaries.summaryAddress(scope, "task-1");
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "summary" });
  const control = order.taskWatchControlAddress(scope);
  const priorControl = (await cache.readCached(control)).value;
  // Deliberately unowned public local write, not a tagged copy or raw reply.
  expect(await summaries.writeSummaryTask(scope, nativeFloors("te-new-blind"), await order.nextTaskRead())).toBe(true);
  const afterBlind = await normalizedNative();
  const afterControl = (await cache.readCached(control)).value;
  expect(afterControl.watchRetirementEpoch).not.toBe(priorControl.watchRetirementEpoch);
  expect(afterControl.blindRetirementEpoch).not.toBe(priorControl.blindRetirementEpoch);
  expect(afterBlind.task.watch_started_after).toBe("te-new-blind");
  // Family-selector witness through the actual public writer. It is not a
  // list membership test and does not rebind this old read to a new query.
  expect(await summaries.writeSummaryTask(scope, nativeSummary({ unread_count: 9,
    read_through: "tc-zz", dismissed_through: "te-zz" }), contentRead.readOrder,
  { contentRead, rawObservation: true })).toBe(true);
  const held = await normalizedNative();
  expect(held.task.watch_started_after).toBe("te-new-blind");
  expect(held.task).toMatchObject({ read_through: "tc-zz", dismissed_through: "te-zz" });
  expect(held.task).not.toHaveProperty("__task_watch_cache");
  expect(held.watch_read_order).toBeUndefined();
  const watch = await import("../src/core/taskWatchAuthority.js");
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ uncertain: false, eligible: true });
  expect(held.watch_uncertain).toBe(true);
});

it.each([["tc-a-b", "te-a-c"], ["tc-\ue000", "te-\u{10000}"], [null, ""]])(
  "advances canonical suffix floor %j to %j independently of dispatch order", async (before, after) => {
    await land(3, { ...row(before, 7), watched: true });
    const ticket = await summaries.beginSummaryQuery(scope, { states: ["open"] }, 1);
    await summaries.landSummaryPage(ticket, { summary: true, basis: "B3", tasks: [{ ...row(after, 0), watched: true }] }, { read: 1 });
    const task = (await summaries.readSummaryQuery(scope, {})).tasks[0];
    for (const field of fields) expect(task[field]).toBe(after);
    expect(task.unread_count).toBe(0);
  },
);

it("uses fresh counters for equal-suffix markers with different prefixes", async () => {
  await land(1, { ...row("tc-A", 1), updated_at: "2026-10-29T00:00:01Z" });
  await land(2, { ...row("te-A", 2), updated_at: "2026-10-29T00:00:02Z" });
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).toMatchObject({ read_through: "tc-A", unread_count: 2 });
  expect(held.stale).toBe(false);
});

it("selects the counters whose combined read/watch floor covers crossed fields", async () => {
  await land(1, { ...row(null, 5), read_through: "tc-10", watch_started_after: "te-15" });
  await land(2, { ...row(null, 4), read_through: "tc-20", watch_started_after: "te-05" });
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).toMatchObject({ read_through: "tc-20", watch_started_after: "te-05", unread_count: 4 });
  expect(held.stale).toBe(false);
});

it("requires refresh when retaining older counters against a newer content snapshot", async () => {
  await land(1, { watched: true, ...row("tc-20", 1), watch_started_after: "", updated_at: "2026-10-29T00:00:01Z" });
  const display = await rememberTaskUnreadDisplay(scope, (await summaries.readSummaryQuery(scope, {})).tasks[0]);
  await land(2, { watched: true, ...row("tc-10", 4), watch_started_after: "", updated_at: "2026-10-29T00:00:02Z" });
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).not.toHaveProperty("unread_count");
  expect(display(held.tasks[0])).toBe(1);
  expect(held.stale).toBe(true);
});

it("keeps attention counter authority separate from the newer watch floor", async () => {
  await land(1, { watched: true, ...row(null, 2), read_through: "tc-20", watch_started_after: "te-05",
    unread_mentions: 0, unread_agent_comments: 0, updated_at: "2026-10-29T00:00:01Z" });
  await land(2, { watched: true, ...row(null, 0), read_through: "tc-10", watch_started_after: "te-30",
    unread_mentions: 3, unread_agent_comments: 3, updated_at: "2026-10-29T00:00:02Z" });
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).toMatchObject({ read_through: "tc-20", watch_started_after: "te-30",
    unread_count: 0 });
  expect(held.tasks[0]).not.toHaveProperty("unread_mentions");
  expect(held.tasks[0]).not.toHaveProperty("unread_agent_comments");
  expect(held.stale).toBe(true);
});

it("ends the watch lineage on authoritative unwatch and rejects its later resurrection", async () => {
  await land(1, { ...row("tc-10", 1), watched: true, watch_started_after: "te-30" });
  const ticket = await summaries.beginSummaryQuery(scope, { states: ["open"] }, 1);
  const unwatched = { id: "task-1", title: "One", watched: false, read_through: "tc-10", dismissed_through: "tc-10" };
  await land(2, unwatched); await land(3, unwatched);
  let held = await summaries.readSummaryQuery(scope, {});
  expect(held.stale).toBe(false);
  expect(held.tasks[0]).not.toHaveProperty("watch_started_after");
  expect(held.tasks[0]).not.toHaveProperty("unread_count");
  await summaries.landSummaryPage(ticket, { summary: true, basis: "B1", tasks: [
    { ...row("tc-10", 1), watched: true, watch_started_after: "te-30" },
  ] }, { read: 1 });
  held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0].watched).toBe(false); expect(held.stale).toBe(true);
  await land(4, { ...row("tc-10", 2), watched: true, watch_started_after: "" });
  held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).toMatchObject({ watched: true, watch_started_after: "", unread_count: 2 });
  expect(held.stale).toBe(false);
});
