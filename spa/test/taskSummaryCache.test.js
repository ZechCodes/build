import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, summaries;
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
});

it("canonicalizes sets and aliases while preserving opaque ID and non-ASCII label case", () => {
  const a = { states: ["open", "open"], labels: ["API", "É"], statuses: ["In review"], assignees: [{ kind: "agent", agent_id: "AgentA" }, "none"] };
  const b = { states: ["open"], labels: ["É", "api"], statuses: ["in_review"], assignees: ["none", { agent_id: "AgentA", kind: "agent" }] };
  expect(summaries.summaryQueryAddress(scope, a)).toEqual(summaries.summaryQueryAddress(scope, b));
  expect(summaries.summaryQueryAddress(scope, a)).not.toEqual(summaries.summaryQueryAddress(scope, { ...b, labels: ["é", "api"] }));
  expect(summaries.summaryQueryAddress(scope, a)).not.toEqual(summaries.summaryQueryAddress(scope, { ...b, assignees: [{ kind: "agent", agent_id: "agenta" }, "none"] }));
  expect(summaries.summaryQueryAddress(scope, { states: [] })).toEqual(summaries.summaryQueryAddress(scope, {}));
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

it("retains newer rows and marker authority when an older page enters another query", async () => {
  await land(await open(query, 3), page([row("task-1", { title: "new", read_through: "tc-03", watch_started_after: "te-02", dismissed_through: "te-04", unread_count: 0 })]));
  const other = { states: ["closed"] };
  await land(await open(other, 1), page([row("task-1", { title: "old", read_through: "tc-01", unread_count: 7 })]));
  expect((await summaries.readSummaryQuery(scope, other)).tasks[0]).toMatchObject({ title: "new", read_through: "tc-03", dismissed_through: "te-04", unread_count: 0 });
});

it("merges identities independently and never writes over an opened detail", async () => {
  const detail = { deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-task", sub: "task-1" };
  await cache.writeCached(detail, { task: { id: "task-1", body: "full body", dependencies: { waiting_on: ["blocker"] } }, timeline: ["history"] });
  await land(await open(), page([row("task-1", { summary_truncated: true })], { next_cursor: "a", identities: { A: { name: "A" } } }));
  const ticket = await open({ states: ["closed"] }, 2);
  await land(ticket, page([], { identities: { B: { name: "B" } } }));
  expect((await summaries.readSummaryQuery(scope, query)).identities).toEqual({ A: { name: "A" }, B: { name: "B" } });
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
