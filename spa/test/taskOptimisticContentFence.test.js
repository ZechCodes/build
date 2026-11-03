import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import fixture from "../../fixtures/api/v1/tasks.read_through.json";

const scope = { deviceId: "optimistic-content-task", projectId: "proj-1" };
const taskId = "moving-task";
const rawTask = (extra = {}) => ({ ...structuredClone(fixture.result.task), id: taskId,
  title: "Server before move", body: "Accepted body", status: "backlog", ...extra });
const deferred = () => {
  let resolve;
  const promise = new Promise((yes) => { resolve = yes; });
  return { promise, resolve };
};
let cache, tracker, content, summary, fence;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  content = await import("../src/core/taskContentAuthority.js");
  summary = await import("../src/core/taskSummaryCache.js");
  fence = await import("../src/core/taskMutationFence.js");
});
afterEach(() => {
  vi.restoreAllMocks();
  fence.resetTaskMutationFences();
});

const address = (id = taskId) => tracker.taskAddress(scope.deviceId, scope.projectId, id);
const normalized = async (id = taskId) => (await cache.readCached(summary.summaryAddress(scope, id)))?.value?.task;
const detail = () => tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
const capture = () => content.captureTaskContentRead(address(), { revalidate: "full" });
const publish = (task, contentRead, options = {}) => tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
  tracker.taskRecord(task, [{ id: "incoming-history" }]), { contentRead, ...options });

async function seed() {
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
    tracker.taskRecord(rawTask(), [{ id: "held-history" }]), { contentRead: await capture() });
  expect((await normalized()).__task_content_cache.header.read_order).toBeGreaterThan(0);
  expect((await detail()).task.__task_content_cache.body.read_order).toBeGreaterThan(0);
}

async function retireMove() {
  const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
  await summary.writeSummaryTask(scope, { id: taskId, status: "ready" }, await nextTaskRead());
  const held = await normalized();
  expect(held.status).toBe("ready");
  expect(held.__task_content_cache.retired.header).toEqual(expect.any(String));
  expect(held.__task_content_cache.header).toBeUndefined();
}

function beginMove(id = taskId) {
  const gate = deferred();
  const done = fence.wrapTaskMutationCall(scope.deviceId, "tasks.update", () => gate.promise,
    { projectId: scope.projectId, taskId: id });
  expect(fence.taskMutationsPending(scope.deviceId)).toBeGreaterThan(0);
  return { gate, done };
}

it("keeps an explicit retired header and its body behind a same-task pending read while admitting independent facts", async () => {
  await seed();
  const moving = beginMove();
  try {
    await retireMove();
    const issued = await capture(), onOrdinaryDeferred = vi.fn();
    const incoming = rawTask({ title: "Old server title", body: "Old server body",
      read_through: "te-01K5ZQ8M4T0J7WQ2R6X3YB9C59", dismissed_through: "te-01K5ZQ8M4T0J7WQ2R6X3YB9C58",
      review_summary: { task_id: taskId, version: 2, status: "closed", mode: "pull_request" } });
    await publish(incoming, issued, { onOrdinaryDeferred });
    const held = await normalized(), saved = await detail();
    expect(held.status).toBe("ready");
    expect(held.title).toBe("Server before move");
    expect(held.__task_content_cache.header).toBeUndefined();
    expect(saved.task.body).toBe("Accepted body");
    expect(saved.timeline).toEqual([{ id: "held-history" }]);
    expect(saved.task.read_through).toBe(incoming.read_through);
    expect(saved.task.dismissed_through).toBe(incoming.dismissed_through);
    expect(held.review_summary.version).toBe(2);
    expect(onOrdinaryDeferred).toHaveBeenCalledTimes(1);
  } finally {
    moving.gate.resolve();
    await moving.done;
  }
});

it("keeps a captured pending reply blocked after settlement and accepts a freshly captured recovery", async () => {
  await seed();
  const moving = beginMove();
  await retireMove();
  const issuedDuringMove = await capture();
  moving.gate.resolve();
  await moving.done;
  expect(fence.taskMutationsPending(scope.deviceId)).toBe(0);
  await publish(rawTask({ body: "Pre-ack body" }), issuedDuringMove);
  expect((await normalized()).status).toBe("ready");
  expect((await detail()).task.body).toBe("Accepted body");
  const recovered = rawTask({ title: "Fresh after settlement", status: "ready", body: "Fresh body" });
  await publish(recovered, await capture());
  expect(await normalized()).toMatchObject({ title: recovered.title, status: "ready" });
  expect((await detail()).task.body).toBe("Fresh body");
  expect((await detail()).task.__task_content_cache.body.read_order).toBeGreaterThan(issuedDuringMove.readOrder);
});

it("checks current same-task pending at publication when the capture preceded the mutation", async () => {
  await seed();
  await retireMove();
  const issuedBeforeMove = await capture();
  const moving = beginMove();
  try {
    await publish(rawTask({ body: "Pending publication body" }), issuedBeforeMove);
    expect((await normalized()).status).toBe("ready");
    expect((await detail()).task.body).toBe("Accepted body");
  } finally {
    moving.gate.resolve();
    await moving.done;
  }
});

it("admits a retired header recovery during an unrelated task mutation", async () => {
  await seed();
  await retireMove();
  const moving = beginMove("different-task");
  try {
    await publish(rawTask({ title: "Independent recovery", status: "ready", body: "Independent body" }), await capture());
    expect(await normalized()).toMatchObject({ title: "Independent recovery", status: "ready" });
    expect((await detail()).task.body).toBe("Independent body");
  } finally {
    moving.gate.resolve();
    await moving.done;
  }
});

it("admits a known unretired header while the same task mutation is pending", async () => {
  await seed();
  const moving = beginMove();
  try {
    await publish(rawTask({ title: "Known header update", body: "Known body update" }), await capture());
    expect((await normalized()).title).toBe("Known header update");
    expect((await detail()).task.body).toBe("Known body update");
  } finally {
    moving.gate.resolve();
    await moving.done;
  }
});

it("does not request settlement recovery when a strict timestamp already rejects the pending header", async () => {
  await seed();
  const moving = beginMove();
  try {
    await retireMove();
    const issued = await capture(), onOrdinaryDeferred = vi.fn();
    expect(issued.ticket).not.toBeNull();
    expect(issued.written).toBe(cache.recordWriteOf(await cache.readCached(address())));
    const incoming = rawTask({ title: "Independently older", body: "Independently older body",
      updated_at: "2026-09-18T10:12:00Z" });
    await publish(incoming, issued, { onOrdinaryDeferred });
    expect(await normalized()).toMatchObject({ status: "ready", title: "Server before move" });
    expect((await detail()).task.body).toBe("Accepted body");
    expect(onOrdinaryDeferred).not.toHaveBeenCalled();
  } finally {
    moving.gate.resolve();
    await moving.done;
  }
});

async function retryPublication(retireOwner) {
  await seed();
  cache.setCacheRecoveryTiming({ reopenDelaysMs: [0, 1, 2, 3], openTimeoutMs: 200, restMs: 40 });
  const moving = beginMove();
  await retireMove();
  const issued = await capture(), beforeDetail = await cache.readCached(address());
  const beforeSummary = await cache.readCached(summary.summaryAddress(scope, taskId));
  const { recordKey } = await import("../src/core/idbRecords.js");
  const detailKey = recordKey(address()), originalPut = IDBObjectStore.prototype.put;
  const incoming = rawTask({ read_through: "te-01K5ZQ8M4T0J7WQ2R6X3YB9C59" });
  const attempts = [], notifications = [], observedCommit = [];
  let active = true, committed = false;
  const onOrdinaryDeferred = vi.fn(() => { observedCommit.push(committed); });
  const stop = cache.subscribeCache({ deviceId: scope.deviceId, entityId: scope.projectId }, (changed) => notifications.push(changed));
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (key === detailKey && record?.value?.task?.read_through === incoming.read_through) {
      attempts.push(structuredClone(record.value));
      if (attempts.length === 1) {
        if (retireOwner) active = false;
        throw new DOMException("connection closed during deferred publication", "InvalidStateError");
      }
      this.transaction.addEventListener("complete", () => { committed = true; });
    }
    return originalPut.call(this, record, key);
  });
  try {
    const written = await publish(incoming, issued, { accept: () => active, onOrdinaryDeferred });
    return { written, attempts, notifications, observedCommit, onOrdinaryDeferred, beforeDetail, beforeSummary,
      afterDetail: await cache.readCached(address()), afterSummary: await cache.readCached(summary.summaryAddress(scope, taskId)) };
  } finally {
    stop();
    vi.restoreAllMocks();
    moving.gate.resolve();
    await moving.done;
  }
}

it("does not emit a deferred callback from an aborted attempt when the owner retires before retry", async () => {
  const result = await retryPublication(true);
  expect(result.attempts).toHaveLength(1);
  expect(result.attempts[0].task.read_through).toBe("te-01K5ZQ8M4T0J7WQ2R6X3YB9C59");
  expect(result.written).toBe(false);
  expect(result.afterDetail).toEqual(result.beforeDetail);
  expect(result.afterSummary).toEqual(result.beforeSummary);
  expect(result.notifications).toEqual([]);
  expect(result.onOrdinaryDeferred).not.toHaveBeenCalled();
});

it("emits one deferred callback after a legitimate retry commits both ordinary and independent selections", async () => {
  const result = await retryPublication(false);
  expect(result.attempts).toHaveLength(2);
  expect(result.written).toBe(true);
  expect(result.afterSummary.value.task.status).toBe("ready");
  expect(result.afterDetail.value.task.read_through).toBe("te-01K5ZQ8M4T0J7WQ2R6X3YB9C59");
  expect(result.afterDetail.value.task.body).toBe("Accepted body");
  expect(result.onOrdinaryDeferred).toHaveBeenCalledTimes(1);
  expect(result.observedCommit).toEqual([true]);
});

it("keeps exact-task overlap pending and ignores old settlement after fence replacement", async () => {
  await seed();
  await retireMove();
  const first = beginMove(), second = beginMove();
  first.gate.resolve();
  await first.done;
  expect(fence.taskMutationsPendingFor(scope, taskId)).toBe(1);
  await publish(rawTask(), await capture());
  expect((await normalized()).status).toBe("ready");
  fence.retireTaskMutationFence(scope.deviceId);
  const replacement = beginMove();
  second.gate.resolve();
  await second.done;
  expect(fence.taskMutationsPendingFor(scope, taskId)).toBe(1);
  await publish(rawTask(), await capture());
  expect((await normalized()).status).toBe("ready");
  replacement.gate.resolve();
  await replacement.done;
  expect(fence.taskMutationsPendingFor(scope, taskId)).toBe(0);
  await publish(rawTask({ status: "ready", title: "After all pending owners" }), await capture());
  expect((await normalized()).title).toBe("After all pending owners");
});

it("releases exact-task pending on synchronous throw and rejection without changing their outcomes", async () => {
  const identity = { projectId: scope.projectId, taskId }, failure = new Error("update refused");
  const dispatch = () => {
    expect(fence.taskMutationsPendingFor(scope, taskId)).toBe(1);
    throw failure;
  };
  expect(() => fence.wrapTaskMutationCall(scope.deviceId, "tasks.update", dispatch, identity)).toThrow(failure);
  expect(fence.taskMutationsPendingFor(scope, taskId)).toBe(0);
  const rejected = Promise.reject(failure);
  expect(fence.wrapTaskMutationCall(scope.deviceId, "tasks.update", () => rejected, identity)).toBe(rejected);
  await expect(rejected).rejects.toBe(failure);
  expect(fence.taskMutationsPendingFor(scope, taskId)).toBe(0);
});
