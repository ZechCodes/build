import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let cache, summaries, content;
const scope = { deviceId: "device-891", projectId: "proj-1" };
const query = { states: ["open"] };
const row = (id, extra = {}) => ({ id, number: Number(id.slice(-1)), title: id, updated_at: "2026-10-29T12:00:00Z", ...extra });
const page = (tasks = [], extra = {}) => ({ summary: true, tasks, ...extra });
const open = (filters = query, read = 1) => summaries.beginSummaryQuery(scope, filters, read);
const land = (ticket, answer, read = ticket.generation) => summaries.landSummaryPage(ticket, answer, { read });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  content = await import("../src/core/taskContentAuthority.js");
});

it("canonicalizes sets and aliases while preserving opaque ID and non-ASCII label case", () => {
  const a = { states: ["open", "open"], labels: ["API", "É"], statuses: ["In review"], assignees: [{ kind: "agent", agent_id: "AgentA" }, "none"] };
  const b = { states: ["open"], labels: ["É", "api"], statuses: ["in_review"], assignees: ["none", { agent_id: "AgentA", kind: "agent" }] };
  expect(summaries.summaryQueryAddress(scope, a)).toEqual(summaries.summaryQueryAddress(scope, b));
  expect(summaries.summaryQueryAddress(scope, a)).not.toEqual(summaries.summaryQueryAddress(scope, { ...b, labels: ["é", "api"] }));
  expect(summaries.summaryQueryAddress(scope, a)).not.toEqual(summaries.summaryQueryAddress(scope, { ...b, assignees: [{ kind: "agent", agent_id: "agenta" }, "none"] }));
  expect(summaries.summaryQueryAddress(scope, { states: [] })).toEqual(summaries.summaryQueryAddress(scope, {}));
  expect(summaries.summaryQueryAddress(scope, { label: " api " })).not.toEqual(summaries.summaryQueryAddress(scope, { label: "api" }));
  expect(summaries.summaryQueryAddress(scope, { state: " open " })).not.toEqual(summaries.summaryQueryAddress(scope, { state: "open" }));
});

it("checks action acceptance in the same transaction as invalidation and preserves full-task absences", async () => {
  await land(await open(), page([row("task-1", { watched: true, done_at: "yesterday" })]));
  expect(await summaries.writeSummaryTask(scope, { id: "task-1", title: "rejected" }, 2, { accept: () => false })).toBe(false);
  expect((await summaries.readSummaryQuery(scope, query)).stale).toBe(false);
  const contentRead = await content.captureTaskContentRead(scope);
  const confirmed = row("task-1", { project_id: scope.projectId, title: "confirmed", state: "open",
    status: "ready", body: "Full canonical task body", attachments: [] });
  expect(await summaries.writeSummaryTask(scope, confirmed, contentRead.readOrder, { fullTask: true, contentRead })).toBe(true);
  expect((await summaries.readSummaryQuery(scope, query)).tasks[0]).toMatchObject({ title: "confirmed", watched: false, done_at: null });
});

it("accepts explicit newer review authority on an immediate patch", async () => {
  const review = (version) => ({ task_id: "task-1", version, status: "open" });
  await land(await open(), page([row("task-1", { review_summary: review(1) })]));
  await summaries.writeSummaryTask(scope, { id: "task-1", review_summary: review(2) }, 2);
  expect((await summaries.readSummaryQuery(scope, query)).tasks[0].review_summary.version).toBe(2);
});

it("allows an older cross-query read to advance review version without rolling ordinary fields back", async () => {
  const review = (version) => ({ task_id: "task-1", version, status: "open" });
  const other = { states: ["closed"] };
  // Reserve content provenance before the newer publication, not just a lower query generation.
  const older = await open(other, 1);
  await land(await open(query, 3), page([row("task-1", { title: "new title", review_summary: review(1) })]));
  await land(older, page([row("task-1", { title: "old title", review_summary: review(2) })]));
  expect((await summaries.readSummaryQuery(scope, query)).tasks[0]).toMatchObject({ title: "new title", review_summary: { version: 2 } });
});

it("normalizes entities independently from query membership and reads only cached records", async () => {
  const first = await open();
  await land(first, page([row("task-2"), row("task-1")], { next_cursor: "next", identities: { AgentA: { name: "Ada" } } }));
  const held = await summaries.readSummaryQuery(scope, query);
  expect(held.tasks.map(({ id }) => id)).toEqual(["task-2", "task-1"]);
  expect(held.next_cursor).toBe("next");
  expect(held.complete).toBe(false);
  expect(held).not.toHaveProperty("total");
  expect(held.identities.AgentA.name).toBe("Ada");
  const membership = (await cache.readCached(summaries.summaryQueryAddress(scope, query))).value;
  expect(membership.ids).toEqual(["task-2", "task-1"]);
  expect(membership).not.toHaveProperty("tasks");
});

it("accepts empty continuation, deduplicates rows, and advances only the matching cursor", async () => {
  const first = await open();
  await land(first, page([row("task-2")], { next_cursor: "a" }));
  const next = { ...first, cursor: "a" };
  await land(next, page([], { next_cursor: "b" }), 2);
  expect((await summaries.readSummaryQuery(scope, query)).next_cursor).toBe("b");
  expect(await land(next, page([row("task-1")], { next_cursor: "wrong" }), 3)).toBe(false);
  await land({ ...first, cursor: "b" }, page([row("task-2"), row("task-1")]), 4);
  const held = await summaries.readSummaryQuery(scope, query);
  expect(held.tasks.map(({ id }) => id)).toEqual(["task-2", "task-1"]);
  expect(held.complete).toBe(true);
});

it("preserves membership and cursor during a refresh until a successful first page", async () => {
  await land(await open(), page([row("task-1")], { next_cursor: "old" }));
  const fresh = await open(query, 2);
  const held = await summaries.readSummaryQuery(scope, query);
  expect(held.tasks).toHaveLength(1);
  expect(held.next_cursor).toBe("old");
  expect(held.refreshing).toBe(true);
  await land(fresh, page([]));
  expect((await summaries.readSummaryQuery(scope, query)).tasks).toEqual([]);
  expect((await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.task.id).toBe("task-1");
});

it("rejects stale refresh generations and session acceptance at the write transaction", async () => {
  const stale = await open();
  const current = await open(query, 2);
  expect(await land(stale, page([row("task-1")]))).toBe(false);
  expect(await summaries.landSummaryPage(current, page([row("task-2")]), { read: 2, accept: () => false })).toBe(false);
  expect((await summaries.readSummaryQuery(scope, query)).tasks).toEqual([]);
});

it("keeps column and filter query cursors independent", async () => {
  const ready = { ...query, statuses: ["ready"] };
  const done = { ...query, statuses: ["done"] };
  await land(await open(ready), page([row("task-1")], { next_cursor: "ready-next" }));
  await land(await open(done, 2), page([row("task-2")], { next_cursor: "done-next" }));
  expect((await summaries.readSummaryQuery(scope, ready)).next_cursor).toBe("ready-next");
  expect((await summaries.readSummaryQuery(scope, done)).next_cursor).toBe("done-next");
  expect(await summaries.readSummaryQuery(scope, query)).toBeNull();
});

it("marks another query stale when shared rows change after its membership observation", async () => {
  const ready = { statuses: ["ready"] };
  const done = { statuses: ["done"] };
  await land(await open(ready), page([row("task-1", { status: "ready" })]));
  await land(await open(done, 2), page([row("task-1", { status: "done" })]));
  expect((await summaries.readSummaryQuery(scope, ready)).stale).toBe(true);
  expect((await summaries.readSummaryQuery(scope, done)).stale).toBe(false);
});

it("invalidates earlier absence observations when another query can reveal new members", async () => {
  const ready = { statuses: ["ready"] };
  await land(await open(ready), page([]));
  await land(await open({}, 2), page([row("task-1", { status: "ready" })]));
  expect((await summaries.readSummaryQuery(scope, ready)).stale).toBe(true);
});

it("checks refresh reservation acceptance inside the cache transaction", async () => {
  const ticket = await open();
  await land(ticket, page([row("task-1")], { next_cursor: "a" }));
  await summaries.beginSummaryQuery(scope, query, 2, { accept: () => false });
  const held = await summaries.readSummaryQuery(scope, query);
  expect(held.refreshing).toBe(false);
  expect(held.generation).toBe(ticket.generation);
});

it("merges stronger markers from an older read independently of ordinary fields", async () => {
  const older = await open({}, 1);
  await land(await open(query, 3), page([row("task-1", { title: "held", read_through: "tc-01", unread_count: 5 })]));
  await land(older, page([row("task-1", { title: "old", read_through: "tc-03", unread_count: 0 })]));
  expect((await summaries.readSummaryQuery(scope, query)).tasks[0]).toMatchObject({ title: "held", read_through: "tc-03", unread_count: 0 });
});

it("requires revalidation when stronger marks arrive with an older content snapshot", async () => {
  const older = await open({}, 1);
  await land(await open(query, 3), page([row("task-1", { updated_at: "2026-10-29T13:00:00Z", read_through: "tc-01", unread_count: 5 })]));
  await land(older, page([row("task-1", { updated_at: "2026-10-29T12:00:00Z", read_through: "tc-03", unread_count: 0 })]));
  const held = await summaries.readSummaryQuery(scope, query);
  expect(held.tasks[0]).toMatchObject({ read_through: "tc-03", unread_count: 5 });
  expect(held.stale).toBe(true);
});

it("retains newer rows and marker authority when an older page enters another query", async () => {
  const other = { states: ["closed"] };
  const older = await open(other, 1);
  await land(await open(query, 3), page([row("task-1", { title: "new", read_through: "tc-03", watch_started_after: "te-02", dismissed_through: "te-04", unread_count: 0 })]));
  await land(older, page([row("task-1", { title: "old", read_through: "tc-01", unread_count: 7 })]));
  expect((await summaries.readSummaryQuery(scope, other)).tasks[0]).toMatchObject({ title: "new", read_through: "tc-03", dismissed_through: "te-04", unread_count: 0 });
});

it("merges identities independently and never writes over an opened detail", async () => {
  const detail = { deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-task", sub: "task-1" };
  await cache.writeCached(detail, { task: { id: "task-1", body: "full body", dependencies: { waiting_on: ["blocker"] } }, timeline: ["history"] });
  await land(await open(), page([row("task-1", { summary_truncated: true })], { next_cursor: "a", identities: { A: { name: "A" } } }));
  const ticket = await open({ states: ["closed"] }, 2);
  await land(ticket, page([], { identities: { B: { name: "B" } } }));
  expect((await summaries.readSummaryQuery(scope, query)).identities).toEqual({ A: { name: "A" } });
  expect((await summaries.readSummaryQuery(scope, { states: ["closed"] })).identities).toEqual({ B: { name: "B" } });
  expect((await cache.readCached(detail)).value.timeline).toEqual(["history"]);
  expect((await cache.readCached(detail)).value.task.body).toBe("full body");
});

it("refuses malformed and nonadvancing pages without consuming their cursor", async () => {
  const first = await open();
  await land(first, page([row("task-1")], { next_cursor: "a" }));
  const next = { ...first, cursor: "a" };
  await expect(land(next, page([], { next_cursor: "a" }), 2)).rejects.toThrow();
  await expect(land(next, { tasks: [] }, 2)).rejects.toThrow();
  expect((await summaries.readSummaryQuery(scope, query)).next_cursor).toBe("a");
});

it("keeps rows, identities and continuation unchanged when the page transaction cannot commit", async () => {
  const first = await open();
  await land(first, page([row("task-1")], { next_cursor: "a" }));
  const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
    throw new DOMException("Full quota", "QuotaExceededError");
  });
  try {
    expect(await land({ ...first, cursor: "a" }, page([row("task-2")], { next_cursor: "b", identities: { X: { name: "New" } } }), 2)).toBe(false);
  } finally { put.mockRestore(); }
  const held = await summaries.readSummaryQuery(scope, query);
  expect(held.next_cursor).toBe("a");
  expect(held.tasks.map(({ id }) => id)).toEqual(["task-1"]);
  expect(held.identities).toEqual({});
  expect(await cache.readCached(summaries.summaryAddress(scope, "task-2"))).toBeUndefined();
});

it("fences late pages with an immediate write while retaining query metadata for recovery", async () => {
  const ticket = await open();
  await land(ticket, page([row("task-1")], { next_cursor: "a" }));
  await summaries.writeSummaryTask(scope, row("task-1", { title: "edited", watched: false }), 3);
  expect(await land({ ...ticket, cursor: "a" }, page([row("task-1", { title: "old", watched: true })]), 2)).toBe(false);
  const held = await summaries.readSummaryQuery(scope, query);
  expect(held.tasks[0]).toMatchObject({ title: "edited", watched: false });
  expect(held.stale).toBe(true);
  expect(held.next_cursor).toBe("a");
  const refresh = await open(query, 4);
  expect((await summaries.readSummaryQuery(scope, query)).stale).toBe(true);
  await land(refresh, page([row("task-1", { title: "confirmed", watched: false })]));
  expect((await summaries.readSummaryQuery(scope, query)).stale).toBe(false);
});

it("preserves marker and unread floors even when a later dispatched read carries older marks", async () => {
  await land(await open(), page([row("task-1", { read_through: "tc-03", unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 })]));
  await land(await open(query, 2), page([row("task-1", { read_through: "tc-01", unread_count: 5, unread_mentions: 1, unread_agent_comments: 3 })]));
  expect((await summaries.readSummaryQuery(scope, query)).tasks[0]).toMatchObject({ read_through: "tc-03", unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 });
});

it("does not collapse unsupported selectors or malformed actors into a known query", () => {
  expect(() => summaries.summaryQueryAddress(scope, { unknown_selector: true })).toThrow();
  expect(() => summaries.summaryQueryAddress(scope, { assignees: [{ kind: "agent", agent_id: "A", extra: true }] })).toThrow();
});

it("keeps full action content out of summary records and retains omitted compact fields", async () => {
  await land(await open(), page([row("task-1", { dependencies: { blocked: true, waiting_on_count: 2, blocks_count: 1 }, links: { workspace_ids: ["ws-1"], parent_task_id: null } })]));
  await summaries.writeSummaryTask(scope, { id: "task-1", body: "detail", attachments: ["large"], dependencies: { blocked: false, waiting_on: ["full graph"] }, links: { commits: ["full commit"] } }, 2);
  const held = (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.task;
  expect(held).not.toHaveProperty("body");
  expect(held).not.toHaveProperty("attachments");
  expect(held.dependencies).toEqual({ blocked: false, waiting_on_count: 2, blocks_count: 1 });
  expect(held.links).toEqual({ workspace_ids: ["ws-1"], parent_task_id: null });
});
