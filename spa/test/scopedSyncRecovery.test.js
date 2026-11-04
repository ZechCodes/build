import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

const deviceId = "recovery-device", projectId = "proj-1";
const scope = { kind: "project_tasks", project_id: projectId, filters: { states: ["open"] }, order: "number" };
const row = (id, revision, extra = {}) => ({ kind: "task", key: id, revision, member: true,
  row: { id, project_id: projectId, number: Number(id.slice(-1)), title: id, state: "open", watched: false,
    updated_at: `2026-11-03T12:00:0${revision}Z` }, ...extra });
const wire = (extra) => ({ status: "ok", incarnation: "i", bridge_epoch: "e", ...extra });
const snapshot = (records, extra = {}) => wire({ watermark: "1", cursor: "S", records, ...extra });
const changes = (records) => wire({ head: "4", cursor: "C4", changes: records });
let engineModule, cacheModule, local, queryAddress;
const engines = [];
beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  engineModule = await import("../src/core/scopedSyncEngine.js");
  cacheModule = await import("../src/core/scopedTaskSyncCache.js");
  local = await import("../src/core/localCache.js");
  const { summaryQueryAddress } = await import("../src/core/taskSummaryCache.js");
  queryAddress = summaryQueryAddress({ deviceId, projectId }, scope.filters);
});
afterEach(() => engines.splice(0).forEach((engine) => engine.dispose()));
function fixture(request) {
  const store = cacheModule.createScopedTaskSyncCache({ deviceId, projectId, scope, bridgeIdentity: deviceId });
  const engine = engineModule.createScopedSyncEngine({ request, storeFor: () => store, epoch: () => "e" });
  engine.track(scope); engines.push(engine); return { engine, store };
}
const ids = async () => (await local.readCached(queryAddress))?.value?.ids;

it("recovers entries and exits committed while a multi-page snapshot was being read", async () => {
  const rpc = vi.fn(async (method, params) => {
    if (method === "sync.changes") return changes([
      row("task-1", "4", { seq: "2", member: false, operation: "upsert" }),
      row("task-3", "3", { seq: "3", operation: "upsert" }),
    ]);
    if (!params.page) return snapshot([row("task-1", "1")], { next_page: "page2" });
    expect(await ids()).toBeUndefined();
    return snapshot([row("task-2", "2")]);
  });
  const { engine, store } = fixture(rpc);
  expect(await engine.catchUp(scope)).toBe(true);
  expect(await ids()).toEqual(["task-3", "task-2"]);
  expect((await store.readCursor()).cursor).toBe("C4");
  expect(rpc.mock.calls[2][1].cursor).toBe("S");
});

it("replays a failed real IndexedDB batch from its committed snapshot cursor", async () => {
  const rpc = vi.fn(async (method) => method === "sync.snapshot" ? snapshot([row("task-1", "1")])
    : changes([row("task-2", "2", { seq: "2", operation: "upsert" })]));
  const { engine, store } = fixture(rpc);
  const put = IDBObjectStore.prototype.put;
  let refused = false;
  const failure = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function(value, key) {
    if (!refused && String(key).includes("sync-cursor") && value.value.cursor === "C4") {
      refused = true; throw new DOMException("quota", "QuotaExceededError");
    }
    return put.call(this, value, key);
  });
  try { expect(await engine.catchUp(scope)).toBe(false); }
  finally { failure.mockRestore(); }
  expect((await store.readCursor()).cursor).toBe("S"); expect(await ids()).toEqual(["task-1"]);
  expect(await engine.catchUp(scope)).toBe(true);
  expect(rpc.mock.calls.at(-1)[1].cursor).toBe("S");
  expect(await ids()).toEqual(["task-2", "task-1"]);
});

it("restarts an expired first snapshot lease with no previously stored cursor", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(snapshot([row("task-1", "1")], { next_page: "expired" }))
    .mockResolvedValueOnce(wire({ status: "resync_required", reason: "lease_expired" }))
    .mockResolvedValueOnce(snapshot([row("task-2", "2")])).mockResolvedValueOnce(changes([]));
  const { engine } = fixture(rpc);
  expect(await engine.catchUp(scope)).toBe(true);
  expect(await ids()).toEqual(["task-2"]);
});

it("reopens an invalidated scope with a snapshot even though its cursor was durably saved", async () => {
  let snapshots = 0;
  const rpc = vi.fn(async (method) => {
    if (method === "sync.snapshot") {
      snapshots += 1;
      return snapshot([row("task-1", snapshots === 1 ? "1" : "3")],
        { cursor: snapshots === 1 ? "S" : "S3", watermark: snapshots === 1 ? "1" : "3" });
    }
    return changes(snapshots === 1 ? [row("task-1", "2", { operation: "invalidate", row: null })] : []);
  });
  const first = fixture(rpc);
  expect(await first.engine.catchUp(scope)).toBe(true);
  expect(await first.store.readCursor()).toMatchObject({ cursor: "C4", snapshotRequired: true });
  first.engine.dispose();
  const reopened = fixture(rpc);
  expect(await reopened.engine.catchUp(scope)).toBe(true);
  expect(snapshots).toBe(2);
  expect(await reopened.store.readCursor()).toMatchObject({ snapshotRequired: false });
});


it("snapshots reopened and handed-over views without reusing persisted cursor origins or registration", async () => {
  let snapshots = 0;
  const rpc = vi.fn(async (method) => {
    if (method === "sync.snapshot") {
      snapshots += 1;
      return snapshot([row(`task-${snapshots}`, String(snapshots))], { cursor: `S${snapshots}`, watermark: String(snapshots) });
    }
    return changes([]);
  });
  const first = fixture(rpc);
  expect(await first.engine.catchUp(scope)).toBe(true);
  first.engine.untrack(scope);
  first.engine.track(scope);
  expect(await first.engine.catchUp(scope)).toBe(true);
  expect(snapshots).toBe(2);
  expect(await ids()).toEqual(["task-2"]);
  first.engine.dispose();
  const { syncCursorAddress } = await import("../src/core/scopedTaskSyncCache.js");
  const cursorAddress = syncCursorAddress(deviceId, scope, projectId);
  const persisted = await local.readCached(cursorAddress);
  await local.writeCached(cursorAddress, { ...persisted.value, registration: "opening-2" });
  const before = rpc.mock.calls.length;
  const handedOver = fixture(rpc);
  expect(await handedOver.engine.catchUp(scope)).toBe(true);
  expect(snapshots).toBe(3);
  expect(rpc.mock.calls.slice(before).map(([method]) => method)).toEqual(["sync.snapshot", "sync.changes"]);
  expect(rpc.mock.calls.at(-1)).toMatchObject(["sync.changes", { cursor: "S3" }]);
  expect((await handedOver.store.readCursor()).registration).toBeUndefined();
});

it("keeps concurrent holder and timeout engines on committed cursor CAS without losing rows", async () => {
  const snapshotRecord = row("task-1", "1"); delete snapshotRecord.member;
  const seed = fixture(async (method) => method === "sync.snapshot" ? snapshot([snapshotRecord])
    : wire({ head: "1", cursor: "S", changes: [] }));
  expect(await seed.engine.catchUp(scope)).toBe(true); seed.engine.dispose();
  const gates = {};
  const gate = (name) => new Promise((resolve) => { gates[name] = resolve; });
  const firstPage = wire({ head: "3", cursor: "C2", continuation: "fixed-origin-S", changes: [row("task-2", "2", { seq: "2", operation: "upsert" })] });
  let racing = false;
  const request = (name) => vi.fn(async (method, params) => {
    if (method === "sync.snapshot") return snapshot([snapshotRecord]);
    expect(method).toBe("sync.changes");
    if (!racing) return wire({ head: "1", cursor: "S", changes: [] });
    // The real bridge binds continuation.origin to the initial request S.
    if (params.continuation) {
      if (params.cursor !== "S") throw Object.assign(new Error("MalformedCursor"), { code: "invalid_params" });
      return wire({ head: "3", cursor: "C3", changes: [row("task-3", "3", { seq: "3", operation: "upsert" })] });
    }
    if (params.cursor === "S") { await gate(name); return firstPage; }
    expect(params.cursor).toBe("C3");
    return wire({ head: "4", cursor: "C4", changes: [row("task-4", "4", { seq: "4", operation: "upsert" })] });
  });
  const holderRequest = request("holder"), timeoutRequest = request("timeout");
  const holder = fixture(holderRequest), timeout = fixture(timeoutRequest);
  expect(await holder.engine.catchUp(scope)).toBe(true);
  expect(await timeout.engine.catchUp(scope)).toBe(true);
  racing = true;
  holderRequest.mockClear(); timeoutRequest.mockClear();
  const commits = [];
  for (const writer of [holder.store, timeout.store]) {
    const publish = writer.publish;
    vi.spyOn(writer, "publish").mockImplementation(async (batch) => {
      const saved = await publish(batch);
      if (saved) commits.push(batch.response.cursor);
      return saved;
    });
  }
  const current = holder.engine.catchUp(scope), stale = timeout.engine.catchUp(scope);
  await vi.waitFor(() => { expect(holderRequest).toHaveBeenCalledOnce(); expect(timeoutRequest).toHaveBeenCalledOnce(); });
  gates.holder(); expect(await current).toBe(true);
  gates.timeout(); expect(await stale).toBe(false);
  expect((await timeout.store.readCursor()).cursor).toBe("C3");
  expect(await timeout.engine.catchUp(scope)).toBe(true);
  expect(commits).toEqual(["C2", "C3", "C4"]);
  expect(await ids()).toEqual(["task-4", "task-3", "task-2", "task-1"]);
  expect((await holder.store.readCursor()).cursor).toBe("C4");
});
