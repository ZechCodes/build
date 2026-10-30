import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let summaries, cache, readers;
const scope = { deviceId: "basis-device", projectId: "proj-1" };
const row = (extra = {}) => ({ id: "task-1", number: 1, title: "One", ...extra });
const page = (basis, tasks = [row()], next_cursor) => ({ summary: true, basis, tasks, next_cursor });
const land = async (filters, generation, answer) => {
  const ticket = await summaries.beginSummaryQuery(scope, filters, generation);
  await summaries.landSummaryPage(ticket, answer, { read: generation });
  return ticket;
};
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };

beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  summaries = await import("../src/core/taskSummaryCache.js");
  cache = await import("../src/core/localCache.js");
  readers = await import("../src/core/taskSummaryReader.js");
});

it("keeps same-basis queries valid despite later shared-row and absence observations", async () => {
  await land({}, 1, page("store:opaque-1"));
  await land({ states: ["closed"] }, 2, page("store:opaque-1", []));
  expect((await summaries.readSummaryQuery(scope, {})).stale).toBe(false);
  await land({ states: ["open"] }, 3, page("store:opaque-1"));
  expect((await summaries.readSummaryQuery(scope, {})).stale).toBe(false);
});

it("invalidates older basis observations without parsing or ordering opaque tokens", async () => {
  await land({}, 1, page("store:zz"));
  await land({ states: ["closed"] }, 2, page("store:aa", []));
  expect((await summaries.readSummaryQuery(scope, {})).stale).toBe(true);
  expect((await summaries.readSummaryQuery(scope, { states: ["closed"] })).basis).toBe("store:aa");
});

it("retains query-local relation and ordering provenance outside normalized entities", async () => {
  await land({}, 1, page("B", [row({ matched_memberships: { linked_workspace_id: "ws\u0000" }, order_anchor_ms: 42 })]));
  await land({ states: ["open"] }, 2, page("B"));
  const entity = (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.task;
  expect(entity).not.toHaveProperty("matched_memberships"); expect(entity).not.toHaveProperty("order_anchor_ms");
  expect((await summaries.readSummaryQuery(scope, {})).tasks[0]).toMatchObject({
    matched_memberships: { linked_workspace_id: "ws\u0000" }, order_anchor_ms: 42,
  });
  expect((await summaries.readSummaryQuery(scope, { states: ["open"] })).tasks[0]).not.toHaveProperty("order_anchor_ms");
});

it("rejects changed or malformed continuation basis without consuming cursor", async () => {
  const ticket = await land({}, 1, page("B", [row()], "cursor"));
  expect(await summaries.landSummaryPage({ ...ticket, cursor: "cursor" }, page("C", [], "next"), { read: 2 })).toBe(false);
  await expect(land({ states: ["open"] }, 3, page(42))).rejects.toThrow();
  expect((await summaries.readSummaryQuery(scope, {})).next_cursor).toBe("cursor");
});

it("stops continuation after unsupported refusal until explicit refresh", async () => {
  const error = Object.assign(new Error("unsupported"), { code: "unavailable", retryable: false,
    details: { reason: "task_query_timestamp_unsupported" } });
  const rpc = vi.fn().mockResolvedValueOnce(page("B", [row()], "cursor")).mockRejectedValueOnce(error)
    .mockResolvedValueOnce(page("C"));
  const session = {};
  const reader = readers.createTaskSummaryReader({ scope, filters: {}, callRpc: rpc,
    capabilities: () => ({ listSummaries: true }), sessionOf: () => session });
  await reader.refresh(); await expect(reader.more()).rejects.toBe(error);
  expect(await reader.more()).toBe(false); expect(rpc).toHaveBeenCalledTimes(2);
  expect((await reader.read()).next_cursor).toBe("cursor");
  await reader.refresh(); expect((await reader.read()).basis).toBe("C");
});

it("rejects results issued before pending or acknowledged mutation epoch changes", async () => {
  let epoch = 0; const held = deferred(); const session = {};
  const rpc = vi.fn().mockImplementationOnce(() => held.promise).mockResolvedValue(page("B"));
  const reader = readers.createTaskSummaryReader({ scope, filters: {}, callRpc: rpc,
    capabilities: () => ({ listSummaries: true }), sessionOf: () => session, mutationEpoch: () => epoch });
  const pending = reader.refresh(); await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(1));
  epoch += 1; held.resolve(page("A")); expect(await pending).toBe(false);
  await reader.refresh(); expect((await reader.read()).basis).toBe("B");
});

it("requires authoritative requested anchor and exact relation provenance", async () => {
  const filters = { order: "anchor", linked_workspace_id: "ws-1", tracker_agent_id: "A" };
  const ticket = await summaries.beginSummaryQuery(scope, filters, 1);
  await expect(summaries.landSummaryPage(ticket, page("B"), { read: 1 })).rejects.toThrow();
  const matched_memberships = { linked_workspace_id: "ws-1", tracker_agent_id: "A" };
  await expect(summaries.landSummaryPage(ticket, page("B", [row({ matched_memberships, order_anchor_ms: 1.1 })]), { read: 1 })).rejects.toThrow();
  expect(await summaries.landSummaryPage(ticket, page("B", [row({ matched_memberships, order_anchor_ms: null })]), { read: 1 })).toBe(true);
  expect((await summaries.readSummaryQuery(scope, filters)).tasks[0].order_anchor_ms).toBeNull();
});

it("does not relabel timestamp-protected entity data with a rejected snapshot basis", async () => {
  await land({}, 1, page("B1", [row({ title: "cached", updated_at: "2026-10-29T12:00:10Z" })]));
  const before = (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value;
  await land({}, 2, page("B2", [row({ title: "wire", updated_at: "2026-10-29T12:00:00Z" })]));
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0].title).toBe("cached");
  expect(held.stale).toBe(true);
  const entity = (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value;
  expect(entity.basis).toBeUndefined();
  expect(entity.basis_uncertain).toBe(true);
  expect(entity.task.__task_content_cache.header).toEqual(before.task.__task_content_cache.header);
  expect(held.basis).toBe("B2");
  await land({}, 3, page("B3", [row({ title: "revalidated", updated_at: "2026-10-29T12:00:11Z" })]));
  expect(await summaries.readSummaryQuery(scope, {})).toMatchObject({ basis: "B3", stale: false, tasks: [{ title: "revalidated" }] });
  expect((await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value).toMatchObject({ basis: "B3", basis_uncertain: false });
});

it.each([
  [{ review_summary: { task_id: "task-1", version: 1, status: "open" } }, { review_summary: { task_id: "task-1", version: 2, status: "open" } }],
  [{ read_through: "tc-10", unread_count: 7 }, { read_through: "tc-20", unread_count: 0 }],
])("marks mixed authority from an older different-basis response stale", async (before, after) => {
  const older = await summaries.beginSummaryQuery(scope, { states: ["open"] }, 1);
  const newer = await land({}, 3, page("B1", [row({ title: "held", ...before })]));
  expect(older.contentRead.readOrder).toBeLessThan(newer.contentRead.readOrder);
  await summaries.landSummaryPage(older, page("B2", [row({ title: "old", ...after })]), { read: 1 });
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.tasks[0]).toMatchObject({ title: "held", ...after });
  expect(held.tasks[0].__task_content_cache.header.read_order).toBe(newer.contentRead.readOrder);
  expect(held.stale).toBe(true);
  expect((await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.basis_uncertain).toBe(true);
});

it("does not certify missing normalized entities as an exhausted empty result", async () => {
  await land({}, 1, page("B"));
  await cache.deleteCached([summaries.summaryAddress(scope, "task-1")]);
  const held = await summaries.readSummaryQuery(scope, {});
  expect(held.ids).toEqual(["task-1"]);
  expect(held.stale).toBe(true); expect(held.complete).toBe(false);
  expect(held.missing_ids).toEqual(["task-1"]);
});
