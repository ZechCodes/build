import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const greeting = vi.hoisted(() => ({ paged: false }));
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => ({ tasks: { timelinePaged: greeting.paged } }) }));

let pages, cache;
const entry = (id, at = id) => ({ type: "comment", id, created_at: at, body: id });
const record = (ids, cursor = null) => ({ task: { id: "t1", body: "saved body" },
  timeline: ids.map((id) => entry(id)), has_more: cursor !== null, next_cursor: cursor,
  timeline_read_through: ids.at(-1) || null });
const address = { deviceId: "d1", entityId: "p1", kind: "tracker-task", sub: "t1" };

beforeEach(async () => {
  vi.resetModules(); greeting.paged = false;
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  pages = await import("../src/core/taskTimelinePages.js");
  cache = await import("../src/core/localCache.js");
});

it("uses legacy task selectors until the current greeting advertises paging", () => {
  expect(pages.taskDetailReadParams("d1", "t1")).toEqual({ task_id: "t1" });
  greeting.paged = true;
  expect(pages.taskDetailReadParams("d1", "t1")).toEqual({ task_id: "t1", timeline_paged: true, limit: 50 });
  expect(pages.taskDetailReadParams("d1", "t1", "older")).toEqual({ task_id: "t1", timeline_paged: true, limit: 50, before: "older" });
  greeting.paged = false;
  expect(pages.taskDetailReadParams("d1", "t1", "older")).toBeNull();
});

it("uses a bounded anchor selector only with the current paging capability", () => {
  expect(pages.taskTimelineAnchorReadParams("d1", "t1", "tc-anchor")).toBeNull();
  greeting.paged = true;
  expect(pages.taskTimelineAnchorReadParams("d1", "t1", "tc-anchor"))
    .toEqual({ task_id: "t1", timeline_paged: true, limit: 50, timeline_anchor: "tc-anchor" });
});

it("retains complete-history service attention counters in full-task summary projections", async () => {
  const { fullTaskSummary } = await import("../src/core/taskSummaryProjection.js");
  expect(fullTaskSummary({ id: "t1", unread_count: 7, unread_agent_comments: 3, unread_mentions: 1, unread_service_mentions: 2 }))
    .toMatchObject({ unread_count: 7, unread_agent_comments: 3, unread_mentions: 1, unread_service_mentions: 2 });
});

it("preserves loaded older history and its continuation across an overlapping new head", () => {
  const held = record(["a", "b", "c"], "older-a");
  const incoming = record(["c", "d"], "older-c");
  incoming.timeline[0].body = "edited c";
  const next = pages.mergeTaskTimeline(held, incoming);
  expect(next.timeline.map((row) => row.id)).toEqual(["a", "b", "c", "d"]);
  expect(next.timeline[2].body).toBe("edited c");
  expect(next).toMatchObject({ has_more: true, next_cursor: "older-a" });
});

it("replaces a disjoint head instead of pretending the missing interval is loaded", () => {
  expect(pages.mergeTaskTimeline(record(["a", "b"], "old"), record(["d", "e"], "new")))
    .toMatchObject({ timeline: [entry("d"), entry("e")], next_cursor: "new", has_more: true });
  const complete = pages.mergeTaskTimeline({ ...record(["a", "b"], "old"), timeline_anchor_pages: { removed: { anchor: "removed" } } }, record(["b", "c"]));
  expect(complete.timeline).toEqual([entry("b"), entry("c")]);
  expect(complete.timeline_anchor_pages).toBeUndefined();
});

it("drops paging metadata on a complete legacy reply and can upgrade that cache again", () => {
  const complete = pages.mergeTaskTimeline(record(["b", "c"], "old"), { timeline: [entry("a"), entry("b"), entry("c")] });
  expect(complete.has_more).toBeUndefined(); expect(complete.next_cursor).toBeUndefined();
  const upgraded = pages.mergeTaskTimeline(complete, record(["c", "d"], "older-c"));
  expect(upgraded.timeline.map((row) => row.id)).toEqual(["a", "b", "c", "d"]);
  expect(upgraded).toMatchObject({ has_more: false, next_cursor: null });
});

it("prepends one older page atomically without accepting its task header or body", async () => {
  await cache.writeCached(address, { ...record(["c", "d"], "older-c"),
    task: { ...record([]).task, identities: { current: { name: "Current name" } } },
    timeline_identities: { current: { name: "Historical cached name" } } });
  const incoming = { ...record(["a", "b"], "older-a"), task: { id: "t1", body: "stale body",
    identities: { departed: { name: "Departed author" }, current: { name: "Old name" } } } };
  expect(await pages.prependTaskTimeline(address, "older-c", incoming)).toBe(true);
  const saved = (await cache.readCached(address)).value;
  expect(saved.task.body).toBe("saved body");
  expect(saved.timeline.map((row) => row.id)).toEqual(["a", "b", "c", "d"]);
  expect(saved.next_cursor).toBe("older-a");
  expect(saved.timeline_read_through).toBe("d");
  expect(saved.timeline_identities).toMatchObject({ departed: { name: "Departed author" }, current: { name: "Current name" } });
});

it("rejects older replies after cursor replacement, removal or owner retirement", async () => {
  const incoming = record(["a", "b"]);
  await cache.writeCached(address, record(["d", "e"], "new"));
  expect(await pages.prependTaskTimeline(address, "old", incoming)).toBe(false);
  expect(await pages.prependTaskTimeline(address, "new", incoming, () => false)).toBe(false);
  await cache.deleteCached([address]);
  expect(await pages.prependTaskTimeline(address, "new", incoming)).toBe(false);
});

it("keeps held timeline pagination when a full read loses ordinary content authority", async () => {
  const { captureTaskContentRead } = await import("../src/core/taskContentAuthority.js");
  const { writeTaskRecord } = await import("../src/core/trackerCache.js");
  const original = { id: "t1", project_id: "p1", number: 1, title: "Saved task", body: "Old body", watched: false,
    updated_at: "2026-11-04T10:00:00Z" };
  await cache.writeCached(address, { task: original, timeline: [entry("a")] });
  const contentRead = await captureTaskContentRead(address, { revalidate: "full" });
  await cache.writeCached(address, { task: { ...original, body: "Current body", updated_at: "2026-11-04T11:00:00Z" }, timeline: [entry("current")] });
  await writeTaskRecord("d1", "p1", "t1", { task: original, timeline: [entry("a")], has_more: true,
    next_cursor: "unaccepted-older-cursor", timeline_incarnation: "unaccepted-incarnation" }, { contentRead });
  const saved = (await cache.readCached(address)).value;
  expect(saved.task.body).toBe("Current body");
  expect(saved.timeline).toEqual([entry("current")]);
  expect(saved.next_cursor).toBeUndefined();
  expect(saved.timeline_incarnation).toBeUndefined();
  expect(saved.has_more).toBeUndefined();
});

it("preserves loaded older pages when the scoped journal publishes an overlapping task head", async () => {
  const { createScopedTaskSyncCache } = await import("../src/core/scopedTaskSyncCache.js");
  const writer = createScopedTaskSyncCache({ deviceId: "d1", projectId: "p1", scope: { kind: "task", task_id: "t1" }, bridgeIdentity: "d1" });
  await cache.writeCached(address, record(["a", "b", "c"], "older-a"));
  const row = { ...record(["c", "d"], "older-c"), task: { id: "t1", project_id: "p1", title: "Current header", updated_at: "2026-11-04T12:00:00Z", watched: false } };
  expect(await writer.publish({ snapshot: true, capture: await writer.capture(),
    response: { cursor: "sync-new", incarnation: "synthetic-store", watermark: "2", bridge_epoch: "e1" },
    records: [{ kind: "task_head", key: "t1", revision: "2", row }] })).toBe(true);
  const saved = (await cache.readCached(address)).value;
  expect(saved.timeline.map((entry) => entry.id)).toEqual(["a", "b", "c", "d"]);
  expect(saved).toMatchObject({ next_cursor: "older-a", has_more: true, timeline_read_through: "d" });
});

it("keeps a bounded anchor section separately without inventing a contiguous newest history", async () => {
  await cache.writeCached(address, { ...record(["y", "z"], "before-y"), timeline_incarnation: "store-one" });
  const anchor = { ...record(["a", "b"], "before-a"), timeline_incarnation: "store-one" };
  expect(await pages.keepTaskTimelineAnchor(address, "b", anchor)).toBe(true);
  const saved = (await cache.readCached(address)).value;
  expect(saved.timeline.map((row) => row.id)).toEqual(["y", "z"]);
  expect(saved.next_cursor).toBe("before-y");
  expect(saved.timeline_anchor_pages.b.timeline.map((row) => row.id)).toEqual(["a", "b"]);
  expect(saved.timeline_anchor_pages.b.anchor).toBe("b");
  expect(await pages.keepTaskTimelineAnchor(address, "missing", anchor)).toBe(false);
  expect(await pages.keepTaskTimelineAnchor(address, "b", { ...anchor, timeline_incarnation: "replaced-store" })).toBe(false);
  expect(await pages.keepTaskTimelineAnchor(address, "b", { ...anchor, timeline_incarnation: undefined })).toBe(false);
  expect(await pages.prependTaskTimeline(address, "before-y", { ...anchor, timeline_incarnation: undefined })).toBe(false);
  for (const id of ["one", "two", "three", "four"]) {
    await pages.keepTaskTimelineAnchor(address, id, { ...record([id]), timeline_incarnation: "store-one" });
  }
  const anchors = (await cache.readCached(address)).value.timeline_anchor_pages;
  expect(Object.keys(anchors)).toEqual(["one", "two", "three", "four"]);
  expect(anchors.b).toBeUndefined();
});

it("does not merge loaded history or anchors across a changed store incarnation", () => {
  const held = { ...record(["a", "b"], "older-a"), timeline_incarnation: "store-one", timeline_anchor_pages: { a: { anchor: "a" } } };
  const next = pages.mergeTaskTimeline(held, { ...record(["b", "c"], "older-b"), timeline_incarnation: "store-two" });
  expect(next.timeline.map((row) => row.id)).toEqual(["b", "c"]);
  expect(next.next_cursor).toBe("older-b");
  expect(next.timeline_anchor_pages).toBeUndefined();
});
