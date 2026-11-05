// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, event, task } from "./trackerWireFixture.js";
import { fakeSession } from "./deviceSessionFixture.js";

const deviceId = "pending-content-adapter", projectId = "proj-1", taskId = "task-42";
const scope = { deviceId, projectId };
const held = task({ id: taskId, project_id: projectId, number: 42, title: "Held backlog", body: "Held body",
  status: "backlog", watched: false, read_through: "tc-01", attachments: [], trackers: [],
  review_summary: { task_id: taskId, version: 1, status: "open", mode: "pull_request" } });
const timeline = [event({ id: "held-history", task_id: taskId, at: held.updated_at })];
const answer = (row) => ({ task: row, timeline });
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
let runtime, page, feed;

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="pending-page"></div>';
  runtime = null; page = null; feed = null;
});
afterEach(async () => {
  page?.dispose(); feed?.dispose();
  for (const request of runtime?.requests || []) request.resolve(answer(runtime.serverTask));
  runtime?.changes.resetChangeEvents();
  runtime?.contexts.resetDeviceContexts();
  await Promise.allSettled(runtime?.reads || []);
  vi.restoreAllMocks();
});

async function setup() {
  runtime = {
    local: await import("../src/core/localCache.js"),
    tracker: await import("../src/core/trackerCache.js"),
    summaries: await import("../src/core/taskSummaryCache.js"),
    content: await import("../src/core/taskContentAuthority.js"),
    fence: await import("../src/core/taskMutationFence.js"),
    contexts: await import("../src/core/deviceContexts.js"),
    changes: await import("../src/core/changeEvents.js"),
    requests: [], gets: [], updates: [], reads: [], holdGets: false, serverTask: held,
  };
  const scheduler = await import("../src/core/trailingRead.js"), original = scheduler.trailingRead;
  vi.spyOn(scheduler, "trailingRead").mockImplementation((reader, options) => {
    if (!["readTask", "fetchDetail"].includes(reader.name)) return original(reader, options);
    return original((...args) => {
      const reading = Promise.resolve(reader(...args));
      runtime.reads.push(reading);
      return reading;
    }, options);
  });
  runtime.session = fakeSession(deviceId);
  runtime.session.call.mockImplementation((method) => {
    if (method === "session.hello") return Promise.resolve({
      api_version: "3.24.0", push_events: true, events: ["changes"], capabilities: [],
    });
    if (method === "tasks.update") {
      const request = deferred(); runtime.updates.push(request); runtime.requests.push(request);
      return request.promise;
    }
    if (method !== "tasks.get") return Promise.resolve({});
    const request = deferred(); runtime.gets.push(request); runtime.requests.push(request);
    if (!runtime.holdGets) request.resolve(answer(runtime.serverTask));
    return request.promise;
  });
  runtime.context = runtime.contexts.adoptDeviceSession(runtime.session);
  await runtime.changes.greetBridge(runtime.session.call, {
    deviceId, isCurrent: () => runtime.context.session === runtime.session,
    install: (selection) => {
      const adapter = selection.create(runtime.session.call);
      runtime.contexts.adoptBridgeSelection(runtime.context, selection, adapter);
      return adapter;
    },
  });
  runtime.address = runtime.tracker.taskAddress(deviceId, projectId, taskId);
  runtime.summaryAddress = runtime.summaries.summaryAddress(scope, taskId);
  await runtime.tracker.writeTasksRecord(deviceId, projectId, runtime.tracker.tasksRecord([held], columns()));
  const contentRead = await runtime.content.captureTaskContentRead(runtime.address, { revalidate: "full" });
  await runtime.tracker.writeTaskRecord(deviceId, projectId, taskId, runtime.tracker.taskRecord(held, timeline), { contentRead });
  runtime.actualWriter = runtime.tracker.writeTaskRecord;
  runtime.writer = vi.spyOn(runtime.tracker, "writeTaskRecord");
}

async function mount(kind) {
  await setup();
  if (kind === "feed") {
    const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
    feed = createTrackerTaskDetailsFeed({ ...scope, callRpc: runtime.context.rpc });
    await feed.updateTasks([held]);
    expect(feed.read().get(taskId).task.title).toBe(held.title);
    expect(runtime.gets).toHaveLength(0);
    return;
  }
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  page = mountTaskPage(document.querySelector("#pending-page"), {
    ...scope, taskId, projectKey: `${deviceId}|${projectId}`, callRpc: runtime.context.rpc,
    catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }), navigate: vi.fn(),
  });
  await vi.waitFor(() => expect(runtime.gets).toHaveLength(1));
  await runtime.reads[0];
  await vi.waitFor(() => expect(document.querySelector(".task-review-title h1")?.textContent).toBe(held.title));
}

async function optimisticRetirement() {
  await runtime.content.mergeTaskContentRecordsTogether(scope, [runtime.address, runtime.summaryAddress], (records) => ({
    values: records.map((record) => ({ ...record.value, task: runtime.content.retireTaskContent({
      ...record.value.task, title: "Optimistic ready", status: "ready",
    }, "header") })), retire: true,
  }));
  expect((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task.status).toBe("ready");
}

it("registers only the native page update task key through its RPC and existing refresh", async () => {
  await mount("page");
  runtime.holdGets = true;
  const select = document.querySelector("#task-status");
  select.value = "ready";
  select.dispatchEvent(new Event("change"));
  await vi.waitFor(() => expect(runtime.updates).toHaveLength(1));
  expect(runtime.fence.taskMutationsPendingFor(scope, taskId)).toBe(1);
  expect(runtime.fence.taskMutationsPendingFor(scope, "unrelated-task")).toBe(0);
  runtime.serverTask = { ...held, status: "ready", title: "Acknowledged ready" };
  runtime.updates[0].resolve({ task: runtime.serverTask });
  await vi.waitFor(() => expect(runtime.gets).toHaveLength(2));
  expect(runtime.fence.taskMutationsPendingFor(scope, taskId)).toBe(1);
  runtime.gets[1].resolve(answer(runtime.serverTask));
  await runtime.reads[1];
  await vi.waitFor(() => expect(runtime.fence.taskMutationsPendingFor(scope, taskId)).toBe(0));
  expect(runtime.session.call.mock.calls.filter(([method]) => method === "tasks.update")).toEqual([
    ["tasks.update", { task_id: taskId, status: "ready" }],
  ]);
  expect(runtime.gets).toHaveLength(2);
  expect((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task.status).toBe("ready");
});

it.each(["page", "feed"])("recovers an actual deferred %s full read once its exact update settles without a cache hint", async (kind) => {
  await mount(kind);
  runtime.holdGets = true;
  const mutation = deferred(); runtime.requests.push(mutation);
  const pending = runtime.fence.wrapTaskMutationCall(deviceId, "tasks.update", () => mutation.promise, { projectId, taskId });
  expect(runtime.fence.taskMutationsPendingFor(scope, taskId)).toBe(1);
  const before = runtime.gets.length, readsBefore = runtime.reads.length, writesBefore = runtime.writer.mock.calls.length;
  await optimisticRetirement();
  await vi.waitFor(() => expect(runtime.gets).toHaveLength(before + 1));
  const oldSnapshot = { ...held, read_through: "tc-20",
    review_summary: { task_id: taskId, version: 2, status: "closed", mode: "pull_request" } };
  runtime.gets[before].resolve(answer(oldSnapshot));
  await runtime.reads[readsBefore];
  const deferredTask = (await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task;
  expect({ title: deferredTask.title, status: deferredTask.status, body: deferredTask.body }).toEqual({
    title: "Optimistic ready", status: "ready", body: held.body,
  });
  expect(deferredTask.read_through).toBe("tc-20");
  expect(deferredTask.review_summary.version).toBe(2);
  expect((await runtime.local.readCached(runtime.summaryAddress)).value.task.status).toBe("ready");
  expect(runtime.writer.mock.calls[writesBefore][4].contentRead.localOrdinaryMutation).toMatchObject({ ...scope, taskId, pending: true });
  expect(runtime.gets).toHaveLength(before + 1);
  runtime.serverTask = { ...oldSnapshot, title: "Settled ready", status: "ready", updated_at: "2026-10-30T00:00:00Z" };
  mutation.resolve({ task: runtime.serverTask });
  await pending;
  expect(runtime.fence.taskMutationsPendingFor(scope, taskId)).toBe(0);
  // No cache write, push or public refresh is issued after settlement here.
  await vi.waitFor(() => expect(runtime.gets).toHaveLength(before + 2));
  runtime.gets[before + 1].resolve(answer(runtime.serverTask));
  await runtime.reads[readsBefore + 1];
  const freshTask = (await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task;
  expect({ title: freshTask.title, status: freshTask.status, body: freshTask.body }).toEqual({
    title: "Settled ready", status: "ready", body: held.body,
  });
  const freshRead = runtime.writer.mock.calls.at(-1)[4].contentRead;
  expect(freshRead.localOrdinaryMutation.pending).toBe(false);
  expect(freshTask.__task_content_cache.header.read_order).toBe(freshRead.readOrder);
  expect(freshTask.__task_content_cache.body.read_order).toBe(freshRead.readOrder);
  expect(runtime.gets).toHaveLength(before + 2);
});

async function deferredRead(kind) {
  await mount(kind);
  runtime.holdGets = true;
  const mutation = deferred(); runtime.requests.push(mutation);
  const pending = runtime.fence.wrapTaskMutationCall(deviceId, "tasks.update", () => mutation.promise, { projectId, taskId });
  const before = runtime.gets.length, readsBefore = runtime.reads.length;
  await optimisticRetirement();
  await vi.waitFor(() => expect(runtime.gets).toHaveLength(before + 1));
  return { mutation, pending, before, readsBefore };
}

async function landDeferredRead(turn) {
  runtime.gets[turn.before].resolve(answer({ ...held, read_through: "tc-20",
    review_summary: { task_id: taskId, version: 2, status: "closed", mode: "pull_request" } }));
  await runtime.reads[turn.readsBefore];
  const cached = (await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task;
  expect(cached.status).toBe("ready");
  expect(cached.read_through).toBe("tc-20");
  expect(cached.review_summary.version).toBe(2);
}

async function finishRecovery(turn) {
  await vi.waitFor(() => expect(runtime.gets).toHaveLength(turn.before + 2));
  runtime.serverTask = { ...held, title: "Settled ready", status: "ready", updated_at: "2026-10-30T00:00:00Z" };
  runtime.gets[turn.before + 1].resolve(answer(runtime.serverTask));
  await runtime.reads[turn.readsBefore + 1];
  expect((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task.title).toBe("Settled ready");
  expect(runtime.gets).toHaveLength(turn.before + 2);
}

it.each(["page", "feed"])("recovers a deferred %s read when real commit notification settles before its observer", async (kind) => {
  const turn = await deferredRead(kind);
  let committed = false, observerPending = null;
  // Call the genuine producer/writer. Only observe the callback delivered by
  // its completed transaction; no callback is manufactured by this test.
  runtime.writer.mockImplementation((device, project, id, record, options) => runtime.actualWriter(device, project, id, record, {
    ...options,
    onOrdinaryDeferred: () => {
      observerPending = runtime.fence.taskMutationsPendingFor(scope, taskId);
      options.onOrdinaryDeferred?.();
    },
  }));
  const stop = runtime.local.subscribeCache(runtime.address, () => {
    committed = true;
    turn.mutation.resolve({});
  });
  try {
    await landDeferredRead(turn);
    await turn.pending;
    expect(committed).toBe(true);
    expect(observerPending).toBe(0);
    await finishRecovery(turn);
  } finally { stop(); }
});

it.each(["page", "feed"])("waits for both exact mutations before one %s recovery after rejection", async (kind) => {
  const turn = await deferredRead(kind);
  const other = deferred(); runtime.requests.push(other);
  const overlapping = runtime.fence.wrapTaskMutationCall(deviceId, "tasks.update", () => other.promise, { projectId, taskId });
  const rejected = overlapping.catch((error) => error);
  await landDeferredRead(turn);
  turn.mutation.resolve({});
  await turn.pending;
  expect(runtime.fence.taskMutationsPendingFor(scope, taskId)).toBe(1);
  expect(runtime.gets).toHaveLength(turn.before + 1);
  const error = new Error("Rejected update");
  other.reject(error);
  expect(await rejected).toBe(error);
  expect(runtime.fence.taskMutationsPendingFor(scope, taskId)).toBe(0);
  await finishRecovery(turn);
});

it.each(["page", "feed"])("drops a committed deferred %s recovery after disposal", async (kind) => {
  const turn = await deferredRead(kind);
  await landDeferredRead(turn);
  page?.dispose(); feed?.dispose();
  turn.mutation.resolve({});
  await turn.pending;
  await Promise.allSettled(runtime.reads);
  expect(runtime.fence.taskMutationsPendingFor(scope, taskId)).toBe(0);
  expect(runtime.gets).toHaveLength(turn.before + 1);
  expect((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task.status).toBe("ready");
});

it.each(["page", "feed"])("drops a deferred %s settlement demand after real session replacement", async (kind) => {
  const turn = await deferredRead(kind);
  await landDeferredRead(turn);
  const replacement = fakeSession(deviceId);
  replacement.call.mockResolvedValue({});
  expect(runtime.contexts.adoptDeviceSession(replacement)).toBe(runtime.context);
  expect(runtime.context.session).toBe(replacement);
  turn.mutation.resolve({});
  await turn.pending;
  await Promise.allSettled(runtime.reads);
  expect(replacement.call).not.toHaveBeenCalled();
  expect(runtime.gets).toHaveLength(turn.before + 1);
});
