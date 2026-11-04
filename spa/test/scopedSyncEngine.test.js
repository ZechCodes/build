import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createScopedSyncEngine, SYNC_HEAD_INTERVAL_MS } from "../src/core/scopedSyncEngine.js";

const scope = { kind: "task", task_id: "t1" };
const response = (extra = {}) => ({ status: "ok", incarnation: "i", bridge_epoch: "e", ...extra });
const snapshot = (extra = {}) => response({ watermark: "1", cursor: "opaque-S", records: [{ key: "t1" }], ...extra });
const changes = (extra = {}) => response({ head: "2", cursor: "opaque-C", changes: [], ...extra });
const engines = [];
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
async function fixture(request, initial) {
  let cursor = initial, current = true, epoch = "e", priming = Boolean(initial);
  const primingHead = initial?.appliedHead || "1";
  const store = {
    readCursor: vi.fn(async () => cursor), capture: vi.fn(async () => ({})),
    publish: vi.fn(async ({ response: reply, removed, snapshot }) => {
      cursor = removed ? undefined : { cursor: reply.cursor, incarnation: reply.incarnation,
        head: snapshot ? reply.watermark : reply.head, appliedHead: reply.continuation ? null : snapshot ? reply.watermark : reply.head };
      return true;
    }),
    reset: vi.fn(async () => { cursor = undefined; return true; }),
  };
  const read = (method, params) => priming
    ? Promise.resolve(method === "sync.snapshot" ? snapshot({ cursor: initial.cursor, watermark: primingHead, records: [] })
      : changes({ cursor: initial.cursor, head: primingHead })) : request(method, params);
  const engine = createScopedSyncEngine({ request: read, storeFor: () => store, active: () => current, epoch: () => epoch });
  engines.push(engine); engine.track(scope);
  // Ongoing-mechanism cases start after a real completed snapshot in this
  // same engine. New tracks never receive a production resume escape.
  if (initial) {
    await engine.catchUp(scope);
    cursor = initial; priming = false;
    for (const method of [store.publish, store.capture, store.readCursor]) method.mockClear();
  }
  return { engine, store, cursor: () => cursor, retire: () => { current = false; }, epoch: (next) => { epoch = next; } };
}
beforeEach(() => vi.useFakeTimers());
afterEach(() => { engines.splice(0).forEach((engine) => engine.dispose()); vi.useRealTimers(); });

it("publishes all staged snapshot pages once then catches a mutation after the watermark", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(snapshot({ next_page: "page" }))
    .mockResolvedValueOnce(snapshot({ records: [{ key: "t2" }] })).mockResolvedValueOnce(changes());
  const { engine, store } = await fixture(rpc);
  expect(await engine.catchUp(scope)).toBe(true);
  expect(store.publish).toHaveBeenCalledTimes(2);
  expect(store.publish.mock.calls[0][0]).toMatchObject({ snapshot: true, records: [{ key: "t1" }, { key: "t2" }] });
  expect(rpc.mock.calls.map(([method]) => method)).toEqual(["sync.snapshot", "sync.snapshot", "sync.changes"]);
  expect(rpc.mock.calls[2][1].cursor).toBe("opaque-S");
});

it("does not expose a partial snapshot while a later page is outstanding", async () => {
  const second = deferred();
  const rpc = vi.fn().mockResolvedValueOnce(snapshot({ next_page: "p" })).mockReturnValueOnce(second.promise).mockResolvedValue(changes());
  const { engine, store } = await fixture(rpc); const pending = engine.catchUp(scope);
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(2));
  expect(store.publish).not.toHaveBeenCalled();
  second.resolve(snapshot({ records: [] })); await pending;
});

it("restarts an expired multipage lease without publishing its abandoned rows", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(snapshot({ records: [{ key: "abandoned" }], next_page: "p" }))
    .mockResolvedValueOnce(response({ status: "resync_required", reason: "lease_expired" }))
    .mockResolvedValueOnce(snapshot({ records: [{ key: "fresh" }] })).mockResolvedValueOnce(changes());
  const { engine, store } = await fixture(rpc); await engine.catchUp(scope);
  expect(store.publish.mock.calls[0][0].records).toEqual([{ key: "fresh" }]);
  expect(rpc.mock.calls[2][1]).not.toHaveProperty("page");
});

it("drains fixed-head continuations with the original run cursor and separate committed CAS", async () => {
  const rpc = vi.fn(async (_method, params) => {
    // changes.rs::run_position binds every continuation to run.origin.
    if (params.cursor !== "C0") throw Object.assign(new Error("MalformedCursor"), { code: "invalid_params" });
    return params.continuation ? changes({ cursor: "C2" }) : changes({ cursor: "C1", continuation: "fixed" });
  });
  const { engine, store } = await fixture(rpc, { cursor: "C0" });
  expect(await engine.catchUp(scope)).toBe(true);
  expect(rpc.mock.calls[1][1]).toMatchObject({ cursor: "C0", continuation: "fixed" });
  expect(store.publish.mock.calls.map(([batch]) => batch.expectedCursor)).toEqual(["C0", "C1"]);
});

it("leaves a failed publication cursor available to re-read on the next trigger", async () => {
  const rpc = vi.fn().mockResolvedValue(changes());
  const { engine, store, cursor } = await fixture(rpc, { cursor: "C0" });
  store.publish.mockResolvedValueOnce(false);
  expect(await engine.catchUp(scope)).toBe(false); expect(cursor().cursor).toBe("C0");
  expect(await engine.catchUp(scope)).toBe(true); expect(rpc.mock.calls[1][1].cursor).toBe("C0");
});

it.each(["incarnation_changed", "retention_expired", "over_budget", "lease_expired"])("resnapshots only the affected scope on %s", async (reason) => {
  const rpc = vi.fn().mockResolvedValueOnce(response({ status: "resync_required", reason }))
    .mockResolvedValueOnce(snapshot()).mockResolvedValueOnce(changes());
  const { engine, store } = await fixture(rpc, { cursor: "old" });
  engine.track({ kind: "task", task_id: "other" }); await engine.catchUp(scope);
  expect(store.reset).toHaveBeenCalledTimes(1);
  expect(rpc.mock.calls.every(([, params]) => params.scope.task_id === "t1")).toBe(true);
});

it("drops membership and cursor without resnapshotting a removed scope", async () => {
  const rpc = vi.fn().mockResolvedValue(response({ status: "resync_required", reason: "scope_removed" }));
  const { engine, store, cursor } = await fixture(rpc, { cursor: "old" }); await engine.catchUp(scope);
  expect(store.publish).toHaveBeenCalledWith(expect.objectContaining({ removed: true }));
  expect(cursor()).toBeUndefined(); expect(rpc).toHaveBeenCalledTimes(1);
});

it.each(["session", "epoch", "reply epoch"])("discards an obsolete %s response", async (kind) => {
  const answer = deferred(), rpc = vi.fn(() => answer.promise);
  const { engine, store, retire, epoch } = await fixture(rpc); const pending = engine.catchUp(scope);
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(1));
  if (kind === "session") retire(); if (kind === "epoch") epoch("new");
  answer.resolve(snapshot({ bridge_epoch: kind === "reply epoch" ? "old" : "e" }));
  expect(await pending).toBe(false); expect(store.publish).not.toHaveBeenCalled();
});

it("repairs a lost push at 30 seconds and skips catch-up when the applied head matches", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(response({ head: "2" })).mockResolvedValueOnce(changes())
    .mockResolvedValue(response({ head: "2" }));
  const { store } = await fixture(rpc, { cursor: "C1", appliedHead: "1" });
  await vi.advanceTimersByTimeAsync(SYNC_HEAD_INTERVAL_MS - 1); expect(rpc).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1); expect(store.publish).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(SYNC_HEAD_INTERVAL_MS); expect(store.publish).toHaveBeenCalledTimes(1);
});

it("foreground resume and matching pushes wake immediately; closing stops head checks", async () => {
  const rpc = vi.fn().mockResolvedValue(changes()); const { engine } = await fixture(rpc, { cursor: "C1", appliedHead: "2" });
  await engine.wake((candidate) => candidate.task_id === "other"); expect(rpc).not.toHaveBeenCalled();
  await engine.wake(() => true); expect(rpc).toHaveBeenCalledTimes(1);
  await engine.foreground(); expect(rpc).toHaveBeenCalledTimes(2);
  engine.untrack(scope); await vi.advanceTimersByTimeAsync(SYNC_HEAD_INTERVAL_MS); expect(rpc).toHaveBeenCalledTimes(2);
});

it("captures finite recovery without waiting for a push that arrives during the batch", async () => {
  const held = deferred(); const rpc = vi.fn().mockReturnValueOnce(held.promise).mockResolvedValue(changes());
  const { engine } = await fixture(rpc, { cursor: "C0" });
  const captured = engine.catchUp(scope); await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(1));
  const pushed = engine.wake(() => true); held.resolve(changes());
  expect(await captured).toBe(true); await pushed; expect(rpc).toHaveBeenCalledTimes(2);
});

it("bounds empty snapshot pages without publishing a partial membership", async () => {
  let pages = 0;
  const rpc = vi.fn(async () => {
    pages += 1;
    if (pages > 1100) throw new Error("unbounded page walk");
    return snapshot({ records: [], next_page: `page-${pages}` });
  });
  const { engine, store } = await fixture(rpc);
  expect(await engine.catchUp(scope)).toBe(false);
  expect(rpc.mock.calls.length).toBeLessThanOrEqual(1000);
  expect(store.publish).not.toHaveBeenCalled();
});

it("resnapshots a stale scope on open even when its saved applied head matches", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(response({ head: "2" })).mockResolvedValueOnce(snapshot())
    .mockResolvedValueOnce(changes());
  const { engine, store } = await fixture(rpc, { cursor: "stale-cursor", incarnation: "i", appliedHead: "2", snapshotRequired: true });
  await engine.checkHeads();
  expect(rpc.mock.calls.map(([method]) => method)).toEqual(["sync.head", "sync.snapshot", "sync.changes"]);
  expect(store.publish.mock.calls[0][0]).toMatchObject({ snapshot: true, expectedCursor: "stale-cursor" });
});

it("starts a newly tracked scope with a snapshot before using a persisted cursor in head checks", async () => {
  const rpc = vi.fn(async (method) => method === "sync.snapshot" ? snapshot()
    : method === "sync.changes" ? changes() : response({ head: "1" }));
  const { engine, store } = await fixture(rpc);
  store.readCursor.mockResolvedValueOnce({ cursor: "old-persisted", incarnation: "i", appliedHead: "1", registration: "legacy-opening" });
  await engine.checkHeads();
  expect(rpc.mock.calls.map(([method]) => method)).toEqual(["sync.snapshot", "sync.changes"]);
  expect(rpc.mock.calls.some(([, params]) => params.cursor === "old-persisted")).toBe(false);
  expect(store.publish.mock.calls[0][0].expectedCursor).toBe("old-persisted");
});

it("silently retries a reconciling device scope with backoff outside the captured recovery", async () => {
  const device = { kind: "device" }, onError = vi.fn();
  const fence = Object.assign(new Error("sync_reconciling"), { code: "unavailable", retryable: true });
  const request = vi.fn().mockRejectedValueOnce(fence).mockRejectedValueOnce(fence)
    .mockResolvedValueOnce(snapshot({ records: [] })).mockResolvedValue(changes());
  const store = { readCursor: async () => null, capture: async () => ({}), publish: vi.fn(async () => true) };
  const engine = createScopedSyncEngine({ request, storeFor: () => store, onError });
  engines.push(engine); engine.track(device);
  expect(await engine.catchUp(device)).toBe(false);
  expect(store.publish).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(999); expect(request).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1); expect(request).toHaveBeenCalledTimes(2);
  await vi.advanceTimersByTimeAsync(1999); expect(request).toHaveBeenCalledTimes(2);
  await vi.advanceTimersByTimeAsync(1); expect(store.publish).toHaveBeenCalledTimes(2);
  expect(onError).not.toHaveBeenCalled();
});

it("reports a persistent non-fence failure through the error handler and retires pending retries", async () => {
  const device = { kind: "device" }, onError = vi.fn(), failure = new Error("synthetic read failure");
  const request = vi.fn().mockRejectedValue(failure);
  const engine = createScopedSyncEngine({ request, storeFor: () => ({ readCursor: async () => null,
    capture: async () => ({}) }), onError });
  engines.push(engine); engine.track(device);
  await engine.catchUp(device); expect(onError).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1000); expect(onError).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(2000); expect(onError).toHaveBeenCalledExactlyOnceWith(failure);
  engine.untrack(device);
  await vi.advanceTimersByTimeAsync(30_000); expect(request).toHaveBeenCalledTimes(3);
});

it("resnapshots device leases and preserves large opaque page and change continuations", async () => {
  const device = { kind: "device" }, page = "p".repeat(65_536), continuation = "c".repeat(65_536);
  let position = null;
  const store = { readCursor: async () => position, capture: async () => ({}),
    reset: vi.fn(async () => { position = null; return true; }),
    publish: vi.fn(async ({ response: answer }) => { position = { cursor: answer.cursor }; return true; }) };
  const record = { kind: "project", key: "project-synthetic", revision: "1", row: {
    id: "project-synthetic", project_id: "project-synthetic", name: "project", path: "/synthetic/project" } };
  const request = vi.fn().mockResolvedValueOnce(snapshot({ records: [record], next_page: page }))
    .mockResolvedValueOnce(response({ status: "resync_required", reason: "lease_expired" }))
    .mockResolvedValueOnce(snapshot({ records: [record], next_page: page }))
    .mockResolvedValueOnce(snapshot({ records: [] }))
    .mockResolvedValueOnce(changes({ cursor: "C1", continuation }))
    .mockResolvedValueOnce(changes({ cursor: "C2" }));
  const engine = createScopedSyncEngine({ request, storeFor: () => store });
  engines.push(engine); engine.track(device);
  expect(await engine.catchUp(device)).toBe(true);
  expect(request.mock.calls.map(([method]) => method)).toEqual([
    "sync.snapshot", "sync.snapshot", "sync.snapshot", "sync.snapshot", "sync.changes", "sync.changes"]);
  expect(request.mock.calls[1][1]).toEqual({ scope: device, page });
  expect(request.mock.calls[3][1]).toEqual({ scope: device, page });
  expect(request.mock.calls[5][1]).toEqual({ scope: device, cursor: "opaque-S", continuation });
  expect(store.publish.mock.calls.map(([batch]) => batch.expectedCursor)).toEqual([null, "opaque-S", "C1"]);
  expect(store.reset).toHaveBeenCalledTimes(1);
});
