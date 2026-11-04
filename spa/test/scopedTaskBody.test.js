import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const deviceId = "body-device", projectId = "proj-1", taskId = "task-1";
const detail = { deviceId, entityId: projectId, kind: "tracker-task", sub: taskId };
const head = { id: taskId, project_id: projectId, title: "journal header", updated_at: "2026-11-03T12:00:00Z", watched: false };
let cache, body, mutations;
const read = async () => (await cache.readCached(detail))?.value;
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { resolve, promise }; };

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  body = await import("../src/core/scopedTaskBody.js");
  mutations = await import("../src/core/taskMutationFence.js");
  const sync = await import("../src/core/scopedTaskSyncCache.js");
  const writer = sync.createScopedTaskSyncCache({ deviceId, projectId, scope: { kind: "task", task_id: taskId }, bridgeIdentity: deviceId });
  await writer.publish({ response: { cursor: "c", incarnation: "i", watermark: "1", bridge_epoch: "e" }, snapshot: true,
    capture: await writer.capture(), records: [{ key: taskId, revision: "1", kind: "task_head",
      row: { task: head, timeline: ["journal timeline"], has_more: true } }] });
});

const refresh = (callRpc, active = () => true) => body.refreshScopedTaskBody({ deviceId, projectId, taskId, callRpc, active });
const answer = (extra = {}) => ({ task: { ...head, body: "complete body", attachments: ["attachment"], ...extra }, timeline: ["legacy timeline"] });

it("supplements body fields while preserving the journal header, facts and timeline", async () => {
  const call = vi.fn(async () => answer({ title: "full legacy header" }));
  expect(await refresh(call)).toBe(true);
  expect(call).toHaveBeenCalledWith("tasks.get", { task_id: taskId });
  expect(await read()).toMatchObject({ task: { title: "journal header", body: "complete body", attachments: ["attachment"] },
    timeline: ["journal timeline"], has_more: true });
});

it("rejects a body reply after lock/session ownership is retired", async () => {
  const held = deferred(); let active = true;
  const started = deferred();
  const pending = refresh(() => { started.resolve(); return held.promise; }, () => active);
  await started.promise; active = false; held.resolve(answer());
  expect(await pending).toBe(false);
  expect((await read()).task.body).toBeUndefined();
});

it("rejects a body reply after a newer journal head writes the same detail", async () => {
  const held = deferred(), started = deferred();
  const pending = refresh(() => { started.resolve(); return held.promise; });
  await started.promise;
  await cache.writeCached(detail, { task: { ...head, title: "new head" }, timeline: ["new head"] });
  held.resolve(answer());
  expect(await pending).toBe(false);
  expect(await read()).toMatchObject({ task: { title: "new head" }, timeline: ["new head"] });
});

it("rejects body replies across cache reset without resurrecting removed detail", async () => {
  const held = deferred(), started = deferred();
  const pending = refresh(() => { started.resolve(); return held.promise; });
  await started.promise; await cache.wipeCache(); held.resolve(answer());
  expect(await pending).toBe(false);
  expect(await read()).toBeUndefined();
});

it("defers body reads during pending task mutations and fences mutations begun in flight", async () => {
  const held = deferred(), started = deferred();
  const pending = refresh(() => { started.resolve(); return held.promise; });
  await started.promise;
  const mutation = deferred();
  const writing = mutations.wrapTaskMutationCall(deviceId, "tasks.update", () => mutation.promise, { projectId, taskId });
  const call = vi.fn();
  expect(await refresh(call)).toBe(false); expect(call).not.toHaveBeenCalled();
  held.resolve(answer()); expect(await pending).toBe(false);
  mutation.resolve(); await writing;
  expect((await read()).task.body).toBeUndefined();
});

it("does not attach an old task version's body to the journal header", async () => {
  expect(await refresh(async () => answer({ updated_at: "2026-11-02T12:00:00Z" }))).toBe(false);
  expect((await read()).task.body).toBeUndefined();
});

it("waits for journal convergence before attaching a newer body version", async () => {
  expect(await refresh(async () => answer({ updated_at: "2026-11-04T12:00:00Z" }))).toBe(false);
  expect((await read()).task.body).toBeUndefined();
});

it("persists a body basis so its own cache announcement and a reload do not refetch", async () => {
  await refresh(async () => answer());
  const call = vi.fn(async () => answer());
  expect(await refresh(call)).toBe(false);
  const reloaded = await import("../src/core/scopedTaskBody.js?reload");
  expect(await reloaded.refreshScopedTaskBody({ deviceId, projectId, taskId, callRpc: call })).toBe(false);
  expect(call).not.toHaveBeenCalled();
});
