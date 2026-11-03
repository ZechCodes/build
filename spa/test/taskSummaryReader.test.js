import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, readers;
const scope = { deviceId: "reader-891", projectId: "proj-1" };
const row = (id) => ({ id, number: Number(id.slice(-1)), title: id, watched: false });
const page = (tasks, next_cursor) => ({ summary: true, tasks, ...(next_cursor ? { next_cursor } : {}) });
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
let session, capabilities;
const reader = (callRpc, filters = {}) => readers.createTaskSummaryReader({ scope, filters, callRpc,
  capabilities: () => capabilities, sessionOf: () => session });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  session = {}; capabilities = { listSummaries: true };
  cache = await import("../src/core/taskSummaryCache.js");
  readers = await import("../src/core/taskSummaryReader.js");
});

it("reads one canonical filtered summary page and follows continuation only on demand", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(page([row("t2")], "a")).mockResolvedValueOnce(page([], "b"))
    .mockResolvedValueOnce(page([row("t1")]));
  const view = reader(rpc, { states: ["open"], statuses: ["In review", "ready"], labels: ["UI", "ui"], assignees: [{ kind: "agent", agent_id: "AgentA" }] });
  expect(await view.refresh()).toBe(true);
  expect(rpc).toHaveBeenCalledTimes(1);
  expect(rpc.mock.calls[0]).toEqual(["tasks.list", { project_id: scope.projectId, summary: true, limit: 100,
    states: ["open"], statuses: ["in_review", "ready"], labels: ["ui"], assignees: [{ kind: "agent", agent_id: "AgentA" }] }]);
  expect(await view.more()).toBe(true);
  expect(rpc).toHaveBeenCalledTimes(2);
  expect((await view.read()).next_cursor).toBe("b");
  expect(await view.more()).toBe(true);
  expect((await view.read()).tasks).toHaveLength(2);
  expect(await view.more()).toBe(false);
  expect(rpc).toHaveBeenCalledTimes(3);
});

it("does not dispatch summary mode to a bridge that only advertises legacy paging", async () => {
  const rpc = vi.fn(); capabilities = { listPaged: true };
  expect(await reader(rpc).refresh()).toBe(false);
  expect(rpc).not.toHaveBeenCalled();
});

it("coalesces duplicate continuation presses and preserves cache/cursor on nonretryable failure", async () => {
  const held = deferred();
  const rpc = vi.fn().mockResolvedValueOnce(page([row("t2")], "a")).mockImplementationOnce(() => held.promise)
    .mockRejectedValueOnce(Object.assign(new Error("oversized stored row"), { code: "unavailable", retryable: false }));
  const view = reader(rpc);
  await view.refresh();
  const one = view.more(), two = view.more();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(2));
  held.resolve(page([], "b"));
  expect(await one).toBe(true); expect(await two).toBe(true);
  await expect(view.more()).rejects.toMatchObject({ retryable: false });
  expect((await view.read()).next_cursor).toBe("b");
  expect((await view.read()).tasks).toHaveLength(1);
  expect(rpc).toHaveBeenCalledTimes(3);
});

it("fences an earlier refresh, an obsolete session, and a disposed reader", async () => {
  const old = deferred();
  const rpc = vi.fn().mockImplementationOnce(() => old.promise).mockResolvedValueOnce(page([row("t2")]));
  const view = reader(rpc);
  const first = view.refresh();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(1));
  await view.refresh(); old.resolve(page([row("t1")], "old"));
  expect(await first).toBe(false);
  expect((await view.read()).tasks.map(({ id }) => id)).toEqual(["t2"]);
  const pending = deferred(); rpc.mockImplementationOnce(() => pending.promise);
  const next = view.refresh();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(3));
  session = {}; pending.resolve(page([row("t1")]));
  expect(await next).toBe(false);
  view.dispose(); expect(await view.refresh()).toBe(false);
});

it("keeps board-column page cursors independent", async () => {
  const rpc = vi.fn(async (_method, params) => page([row(params.statuses[0] === "ready" ? "t1" : "t2")], `${params.statuses[0]}-next`));
  const ready = reader(rpc, { states: ["open"], statuses: ["ready"] });
  const done = reader(rpc, { states: ["open"], statuses: ["done"] });
  await Promise.all([ready.refresh(), done.refresh()]);
  expect((await ready.read()).next_cursor).toBe("ready-next");
  expect((await done.read()).next_cursor).toBe("done-next");
  expect(rpc).toHaveBeenCalledTimes(2);
});

it("does not continue a stale membership after an immediate mutation", async () => {
  const rpc = vi.fn().mockResolvedValue(page([row("t1")], "a"));
  const view = reader(rpc);
  await view.refresh();
  const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
  await cache.writeSummaryTask(scope, { id: "t1", title: "edit" }, await nextTaskRead());
  expect(await view.more()).toBe(false);
  expect(rpc).toHaveBeenCalledTimes(1);
});
