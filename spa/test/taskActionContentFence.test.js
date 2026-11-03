import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

const scope = { deviceId: "action-content-fence", projectId: "proj-1" }, taskId = "task-1";

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});

it("keeps another mutation's dispatch-time fence after it settles before a full action acknowledgment", async () => {
  const local = await import("../src/core/localCache.js");
  const tracker = await import("../src/core/trackerCache.js");
  const content = await import("../src/core/taskContentAuthority.js");
  const fence = await import("../src/core/taskMutationFence.js");
  const address = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
  const before = task({ id: taskId, project_id: scope.projectId, status: "ready", body: "Before body" });
  const initialRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId, tracker.taskRecord(before, []), { contentRead: initialRead });
  const summaries = await import("../src/core/taskSummaryCache.js");
  const catalogue = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const listRead = await content.captureTaskContentRead(catalogue, { revalidate: "full" });
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([before], []), { contentRead: listRead });
  const addresses = [address, catalogue, summaries.summaryAddress(scope, taskId)];
  const moved = Promise.withResolvers(), settle = Promise.withResolvers();
  const moving = fence.wrapTaskMutationCall(scope.deviceId, "tasks.update", async () => {
    await content.mergeTaskContentRecordsTogether(scope, addresses, (records) => ({
      values: records.map(({ value }) => {
        const move = (row) => content.retireTaskContent({ ...row, status: "in_progress" }, "header");
        return value.tasks ? { ...value, tasks: value.tasks.map(move) } : { ...value, task: move(value.task) };
      }), retire: true,
    }));
    moved.resolve();
    await settle.promise;
  }, { projectId: scope.projectId, taskId });
  await moved.promise;
  const dispatched = Promise.withResolvers(), acknowledge = Promise.withResolvers();
  const action = fence.wrapTaskMutationCall(scope.deviceId, "tasks.read_through", async () => {
    const actionRead = await content.captureTaskContentRead(address, { revalidate: "full", action: true });
    dispatched.resolve();
    await acknowledge.promise;
    const actions = await import("../src/core/taskWaitingOnCache.js");
    await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId,
      { task: { ...before, title: "Late full action", body: "Late action body" } },
      { contentRead: actionRead, issuedRead: actionRead.readOrder });
  });
  await dispatched.promise;
  expect(fence.taskMutationsPending(scope.deviceId)).toBe(2);
  const beforeSettlement = await local.readCanonicalCachedMany(addresses);
  settle.resolve(); await moving;
  expect(fence.taskMutationsPendingFor(scope, taskId)).toBe(0);
  expect(fence.taskMutationsPending(scope.deviceId)).toBe(1);
  expect(await local.readCanonicalCachedMany(addresses)).toEqual(beforeSettlement);
  acknowledge.resolve(); await action;
  expect((await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId)).task)
    .toMatchObject({ title: before.title, status: "in_progress", body: "Before body" });
  expect((await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0].status).toBe("in_progress");
  expect((await local.readCanonicalCached(summaries.summaryAddress(scope, taskId))).value.task.status).toBe("in_progress");
});

it("keeps a read report's own device-wide fence separate from task-scoped mutation protection", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const content = await import("../src/core/taskContentAuthority.js");
  const fence = await import("../src/core/taskMutationFence.js");
  const address = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
  const before = task({ id: taskId, project_id: scope.projectId, status: "ready", body: "Before body" });
  const initialRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId, tracker.taskRecord(before, []), { contentRead: initialRead });
  await fence.wrapTaskMutationCall(scope.deviceId, "tasks.read_through", async () => {
    expect(fence.taskMutationsPending(scope.deviceId)).toBe(1);
    expect(fence.taskMutationsPendingFor(scope, taskId)).toBe(0);
    const actionRead = await content.captureTaskContentRead(address, { revalidate: "full" });
    expect(actionRead.localOrdinaryMutation.pending).toBe(false);
    const actions = await import("../src/core/taskWaitingOnCache.js");
    await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId,
      { task: { ...before, body: "Accepted action body" } },
      { contentRead: actionRead, issuedRead: actionRead.readOrder });
    expect((await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId)).task.body).toBe("Accepted action body");
  });
});
