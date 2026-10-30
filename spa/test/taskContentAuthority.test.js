import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
let cache, content, order;
const scope = { deviceId: "content-model-900", projectId: "proj-1" };
const address = { deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-task", sub: "task-1" };
const task = (extra = {}) => ({ id: "task-1", project_id: scope.projectId, number: 1, title: "Current", body: "Body",
  state: "open", status: "ready", attachments: [], updated_at: "2026-10-29T13:00:00.000900Z", ...extra });
const read = async () => (await cache.readCached(address))?.value?.task;
async function publish(candidate, headerBarrier) {
  let result;
  await content.mergeTaskContentRecordsTogether(scope, [address], ([held], context) => {
    result = content.reconcileTaskContent(held?.value?.task, candidate,
      { context, address, heldWritten: cache.recordWriteOf(held), headerBarrier });
    return { values: [{ task: result.task }], retire: result.retired };
  });
  return result;
}
const observed = async (raw, revalidate = 'full') => content.observeTaskContent(raw,
  { contentRead: await content.captureTaskContentRead(address, { revalidate }), coverage: 'full' });
beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js"); order = await import("../src/core/taskReadOrder.js");
  content = await import("../src/core/taskContentAuthority.js");
});

it("allocates persisted shared content tickets across independent modules and reopen above a future counter", async () => {
  const floor = Date.now() + 100000; await cache.writeCached(order.TASK_READ_COUNT_ADDRESS, floor);
  const one = await content.captureTaskContentRead(scope);
  vi.resetModules(); const second = await import("../src/core/taskContentAuthority.js");
  const [two, three] = await Promise.all([second.captureTaskContentRead(scope), content.captureTaskContentRead(scope)]);
  expect(new Set([one.readOrder, two.readOrder, three.readOrder]).size).toBe(3);
  expect(Math.min(one.readOrder, two.readOrder, three.readOrder)).toBeGreaterThan(floor);
  expect(one.ticket).toMatchObject(two.ticket);
  vi.resetModules(); const reopened = await import("../src/core/taskContentAuthority.js");
  expect((await reopened.captureTaskContentRead(scope)).readOrder).toBeGreaterThan(Math.max(two.readOrder, three.readOrder));
});

it("does not stamp a later observation onto strictly newer held component bytes", async () => {
  const first = await publish(await observed(task()));
  const clock = first.task.__task_content_cache.body.read_order;
  const result = await publish(await observed(task({ title: "Old", body: "Old", updated_at: "2026-10-29T12:00:00Z" })));
  expect(result.task).toMatchObject({ title: "Current", body: "Body" });
  expect(result.task.__task_content_cache.body.read_order).toBe(clock);
  expect(result.acceptedBody).toBe(false);
});

it("rejects an observation captured before a blind rewrite and rollback even when bytes return to their baseline", async () => {
  await publish(await observed(task())); const pending = await observed(task({ title: "Late", body: "Late" }));
  for (const body of ["Optimistic", "Body"]) await content.mergeTaskContentRecordsTogether(scope, [address], ([held]) => ({
    values: [{ ...held.value, task: content.retireTaskContent({ ...held.value.task, body }, "body") }], retire: true,
  }));
  const key = content.taskFullRefreshKey(await read());
  const result = await publish(pending);
  expect(result.task.body).toBe("Body"); expect(result.acceptedBody).toBe(false);
  expect(content.taskFullRefreshKey(result.task)).toBe(key);
  expect(result.task.__task_content_cache.body).toBeUndefined();
});

it("does not promote ambiguous unknown body from an unguarded action but admits an exact addressed full revalidation", async () => {
  await cache.writeCached(address, { task: task() });
  const action = content.observeTaskContent(task({ title: "Ambiguous", body: "Ambiguous" }),
    { contentRead: await content.captureTaskContentRead(scope), coverage: "full" });
  expect((await publish(action)).task.body).toBe("Body");
  const validated = await publish(await observed(task({ title: "Canonical", body: "Canonical" })));
  expect(validated).toMatchObject({ acceptedHeader: true, acceptedBody: true, needsFullRefresh: false });
  expect(validated.task.body).toBe("Canonical");
});

it("lets a summary header bar an older full body without inventing full-body proof", async () => {
  const old = await observed(task({ title: "Old", body: "Old" }));
  const summaryRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const fragment = content.taskContentFragment(scope), records = await cache.readCachedMany(fragment.addresses);
  const header = content.reconcileTaskContent(undefined, content.observeTaskContent(task({ title: "New header" }),
    { contentRead: summaryRead, coverage: "summary" }), { context: fragment.context(records) }).task;
  const result = await publish(old, header);
  expect(result.acceptedBody).toBe(false);
  expect(result.needsFullRefresh).toBe(true);
  expect(result.task?.__task_content_cache?.body).toBeUndefined();
});

it("rejects a pre-reset generation ticket and never adopts its ordinary proof after storage recreates", async () => {
  const pending = await observed(task()); await cache.wipeCache();
  await content.captureTaskContentRead(scope);
  const result = await publish(pending);
  expect(result.acceptedHeader).toBe(false); expect(result.acceptedBody).toBe(false);
  expect(result.task?.__task_content_cache?.body).toBeUndefined();
});
