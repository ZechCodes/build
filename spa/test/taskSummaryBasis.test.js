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
