import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createScopedSyncEngine, SYNC_HEAD_INTERVAL_MS } from "../src/core/scopedSyncEngine.js";

const scope = { kind: "task", task_id: "t1" };
const response = (extra = {}) => ({ status: "ok", incarnation: "i", bridge_epoch: "e", ...extra });
const snapshot = (extra = {}) => response({ watermark: "1", cursor: "opaque-S", records: [{ key: "t1" }], ...extra });
const changes = (extra = {}) => response({ head: "2", cursor: "opaque-C", changes: [], ...extra });
const engines = [];
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
function fixture(request, initial) {
  let cursor = initial, current = true, epoch = "e";
  const store = {
    readCursor: vi.fn(async () => cursor), capture: vi.fn(async () => ({})),
    publish: vi.fn(async ({ response: reply, removed, snapshot }) => {
      cursor = removed ? undefined : { cursor: reply.cursor, incarnation: reply.incarnation,
        head: snapshot ? reply.watermark : reply.head, appliedHead: reply.continuation ? null : snapshot ? reply.watermark : reply.head };
      return true;
    }),
    reset: vi.fn(async () => { cursor = undefined; return true; }),
  };
  const engine = createScopedSyncEngine({ request, storeFor: () => store, active: () => current, epoch: () => epoch });
  engines.push(engine); engine.track(scope);
  return { engine, store, cursor: () => cursor, retire: () => { current = false; }, epoch: (next) => { epoch = next; } };
}
beforeEach(() => vi.useFakeTimers());
afterEach(() => { engines.splice(0).forEach((engine) => engine.dispose()); vi.useRealTimers(); });

it("publishes all staged snapshot pages once then catches a mutation after the watermark", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(snapshot({ next_page: "page" }))
    .mockResolvedValueOnce(snapshot({ records: [{ key: "t2" }] })).mockResolvedValueOnce(changes());
  const { engine, store } = fixture(rpc);
  expect(await engine.catchUp(scope)).toBe(true);
  expect(store.publish).toHaveBeenCalledTimes(2);
  expect(store.publish.mock.calls[0][0]).toMatchObject({ snapshot: true, records: [{ key: "t1" }, { key: "t2" }] });
  expect(rpc.mock.calls.map(([method]) => method)).toEqual(["sync.snapshot", "sync.snapshot", "sync.changes"]);
  expect(rpc.mock.calls[2][1].cursor).toBe("opaque-S");
});

it("does not expose a partial snapshot while a later page is outstanding", async () => {
  const second = deferred();
  const rpc = vi.fn().mockResolvedValueOnce(snapshot({ next_page: "p" })).mockReturnValueOnce(second.promise).mockResolvedValue(changes());
  const { engine, store } = fixture(rpc); const pending = engine.catchUp(scope);
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(2));
  expect(store.publish).not.toHaveBeenCalled();
  second.resolve(snapshot({ records: [] })); await pending;
});

it("restarts an expired multipage lease without publishing its abandoned rows", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(snapshot({ records: [{ key: "abandoned" }], next_page: "p" }))
    .mockResolvedValueOnce(response({ status: "resync_required", reason: "lease_expired" }))
    .mockResolvedValueOnce(snapshot({ records: [{ key: "fresh" }] })).mockResolvedValueOnce(changes());
  const { engine, store } = fixture(rpc); await engine.catchUp(scope);
  expect(store.publish.mock.calls[0][0].records).toEqual([{ key: "fresh" }]);
  expect(rpc.mock.calls[2][1]).not.toHaveProperty("page");
});

it("drains fixed-head continuations using the committed opaque cursor", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(changes({ cursor: "C1", continuation: "fixed" })).mockResolvedValueOnce(changes({ cursor: "C2" }));
  const { engine } = fixture(rpc, { cursor: "C0" }); await engine.catchUp(scope);
  expect(rpc.mock.calls[1][1]).toMatchObject({ cursor: "C1", continuation: "fixed" });
});

it("leaves a failed publication cursor available to re-read on the next trigger", async () => {
  const rpc = vi.fn().mockResolvedValue(changes());
  const { engine, store, cursor } = fixture(rpc, { cursor: "C0" });
  store.publish.mockResolvedValueOnce(false);
  expect(await engine.catchUp(scope)).toBe(false); expect(cursor().cursor).toBe("C0");
  expect(await engine.catchUp(scope)).toBe(true); expect(rpc.mock.calls[1][1].cursor).toBe("C0");
});

it.each(["incarnation_changed", "retention_expired", "over_budget", "lease_expired"])("resnapshots only the affected scope on %s", async (reason) => {
  const rpc = vi.fn().mockResolvedValueOnce(response({ status: "resync_required", reason }))
    .mockResolvedValueOnce(snapshot()).mockResolvedValueOnce(changes());
  const { engine, store } = fixture(rpc, { cursor: "old" });
  engine.track({ kind: "task", task_id: "other" }); await engine.catchUp(scope);
  expect(store.reset).toHaveBeenCalledTimes(1);
  expect(rpc.mock.calls.every(([, params]) => params.scope.task_id === "t1")).toBe(true);
});

it("drops membership and cursor without resnapshotting a removed scope", async () => {
  const rpc = vi.fn().mockResolvedValue(response({ status: "resync_required", reason: "scope_removed" }));
  const { engine, store, cursor } = fixture(rpc, { cursor: "old" }); await engine.catchUp(scope);
  expect(store.publish).toHaveBeenCalledWith(expect.objectContaining({ removed: true }));
  expect(cursor()).toBeUndefined(); expect(rpc).toHaveBeenCalledTimes(1);
});

it.each(["session", "epoch", "reply epoch"])("discards an obsolete %s response", async (kind) => {
  const answer = deferred(), rpc = vi.fn(() => answer.promise);
  const { engine, store, retire, epoch } = fixture(rpc); const pending = engine.catchUp(scope);
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(1));
  if (kind === "session") retire(); if (kind === "epoch") epoch("new");
  answer.resolve(snapshot({ bridge_epoch: kind === "reply epoch" ? "old" : "e" }));
  expect(await pending).toBe(false); expect(store.publish).not.toHaveBeenCalled();
});

it("repairs a lost push at 30 seconds and skips catch-up when the applied head matches", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(response({ head: "2" })).mockResolvedValueOnce(changes())
    .mockResolvedValue(response({ head: "2" }));
  const { store } = fixture(rpc, { cursor: "C1", appliedHead: "1" });
  await vi.advanceTimersByTimeAsync(SYNC_HEAD_INTERVAL_MS - 1); expect(rpc).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1); expect(store.publish).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(SYNC_HEAD_INTERVAL_MS); expect(store.publish).toHaveBeenCalledTimes(1);
});

it("foreground resume and matching pushes wake immediately; closing stops head checks", async () => {
  const rpc = vi.fn().mockResolvedValue(changes()); const { engine } = fixture(rpc, { cursor: "C1", appliedHead: "2" });
  await engine.wake((candidate) => candidate.task_id === "other"); expect(rpc).not.toHaveBeenCalled();
  await engine.wake(() => true); expect(rpc).toHaveBeenCalledTimes(1);
  await engine.foreground(); expect(rpc).toHaveBeenCalledTimes(2);
  engine.untrack(scope); await vi.advanceTimersByTimeAsync(SYNC_HEAD_INTERVAL_MS); expect(rpc).toHaveBeenCalledTimes(2);
});

it("captures finite recovery without waiting for a push that arrives during the batch", async () => {
  const held = deferred(); const rpc = vi.fn().mockReturnValueOnce(held.promise).mockResolvedValue(changes());
  const { engine } = fixture(rpc, { cursor: "C0" });
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
  const { engine, store } = fixture(rpc);
  expect(await engine.catchUp(scope)).toBe(false);
  expect(rpc.mock.calls.length).toBeLessThanOrEqual(1000);
  expect(store.publish).not.toHaveBeenCalled();
});

it("resnapshots a stale scope on open even when its saved applied head matches", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(response({ head: "2" })).mockResolvedValueOnce(snapshot())
    .mockResolvedValueOnce(changes());
  const { engine, store } = fixture(rpc, { cursor: "stale-cursor", incarnation: "i", appliedHead: "2", snapshotRequired: true });
  await engine.checkHeads();
  expect(rpc.mock.calls.map(([method]) => method)).toEqual(["sync.head", "sync.snapshot", "sync.changes"]);
  expect(store.publish.mock.calls[0][0]).toMatchObject({ snapshot: true, expectedCursor: "stale-cursor" });
});
