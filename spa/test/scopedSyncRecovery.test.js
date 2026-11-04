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
