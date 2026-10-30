/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

let blockedStatus, enteredWrite, writeGate, composerOptions;
// Default execution retains the original fixture and assertion path. A unique
// opt-in artifact survives the runner suppressing console output for passes.
const tracePath = process.env.TASK891_MOVE_TRACE || null;
let publicationTrace, traceSnapshots, traceStops, traceAddresses, summaryTraceSpy;
const rowTrace = (record) => ({ exists: Boolean(record), read_order: record?.read_order,
  tasks: (record?.tasks || (record?.task ? [record.task] : [])).map((row) => ({
    id: row.id, status: row.status, title: row.title, content: row.__task_content_cache,
  })) });
const tracePublication = (entry) => {
  const recorded = structuredClone(entry);
  publicationTrace.push(recorded);
  return recorded;
};
const traceWriter = (name, writer, describe) => async (...args) => {
  tracePublication({ event: "writer-start", name, ...describe(args) });
  try {
    const result = await writer(...args);
    tracePublication({ event: "writer-return", name, result });
    return result;
  } catch (error) {
    tracePublication({ event: "writer-error", name, message: error.message });
    throw error;
  }
};
const mockTrackerCache = () => vi.doMock("../src/core/trackerCache.js", async (actual) => {
  const source = await actual();
  const cacheExports = { ...source, writeTasksQueryRecord: async (...args) => {
    if (blockedStatus && args[3].tasks.some((row) => row.status === blockedStatus)) {
      blockedStatus = null;
      enteredWrite = true;
      await writeGate.promise;
    }
    return source.writeTasksQueryRecord(...args);
  } };
  if (!tracePath) return cacheExports;
  return { ...cacheExports,
    writeTasksRecord: traceWriter("writeTasksRecord", source.writeTasksRecord, (args) => rowTrace(args[2])),
    writeTaskRecord: traceWriter("writeTaskRecord", source.writeTaskRecord, (args) => rowTrace(args[3])),
    writeTasksQueryRecord: traceWriter("writeTasksQueryRecord", cacheExports.writeTasksQueryRecord, (args) => rowTrace(args[3])),
    writeTaskListRecords: traceWriter("writeTaskListRecords", source.writeTaskListRecords, (args) => ({
      entries: args[2].map(({ address, record, contentRead }) => ({ address, ...rowTrace(record), contentRead })),
    })),
  };
});
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ tasks: {} }), watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/notify.js", () => ({ notifyError() {} }));
vi.mock("../src/core/taskComposer.js", () => ({
  openTaskComposer: (_host, options) => { composerOptions = options; return { isOpen: () => true, close() {} }; },
}));

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const scope = { deviceId: "move-device", projectId: "move-project" };
let pane, aggregate, fence, host, mutation, oldAnswer, rpc, paneCall, summaries, cache;

const mountFixture = async () => {
  if (tracePath) { publicationTrace = []; traceSnapshots = []; traceStops = []; traceAddresses = []; summaryTraceSpy = null; }
  vi.resetModules();
  // Keep the partial mock's actual cache and content-authority modules in
  // the same lifetime as the fresh pane and its cache subscriptions.
  mockTrackerCache();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="host"></div>';
  host = document.querySelector("#host");
  blockedStatus = null; enteredWrite = false; writeGate = deferred();
  composerOptions = null;
  mutation = deferred(); oldAnswer = deferred();
  fence = await import("../src/core/taskMutationFence.js");
  const held = task({ id: "move-task", project_id: scope.projectId, title: "Move me",
    review_summary: { mode: "snapshot", status: "open" } });
  const tracker = await import("../src/core/trackerCache.js");
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [held], columns: columns() });
  cache = await import("../src/core/localCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
  const order = await nextTaskRead();
  const ticket = await summaries.beginSummaryQuery(scope, {}, order);
  await summaries.landSummaryPage(ticket, { summary: true, basis: "before-move", next_cursor: "old-next",
    identities: {}, tasks: [{ id: held.id, project_id: scope.projectId, title: held.title, number: 1,
      state: "open", status: "backlog", review_summary: held.review_summary }] }, { read: order });
  paneCall = vi.fn(async (method) => {
    if (method === "tasks.list") return { tasks: [held] };
    if (method === "tasks.get") return { task: held, timeline: [] };
    if (method === "tasks.update") return mutation.promise;
    return {};
  });
  if (tracePath) installPublicationTrace(tracker);
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(host, { ...scope, projectKey: "move-device|move-project", defaultView: "board", callRpc: paneCall,
    feed: () => ({ items: [], projects: [], workspaces: [] }), catalog: () => ({ providers: [] }), navigate() {} });
  await vi.waitFor(() => expect(host.querySelector('[data-column="backlog"] .task-card')).not.toBeNull(), { timeout: 10_000 });
  const { createTaskAggregateReader } = await import("../src/core/taskAggregateReads.js");
  rpc = vi.fn().mockResolvedValueOnce({ project_id: scope.projectId, unfinished_open_count: 7,
    watched_unread_total: 3, agents: [], basis: "first" }).mockImplementationOnce(() => oldAnswer.promise);
  aggregate = createTaskAggregateReader({ scope, callRpc: rpc, capabilities: () => ({ counts: true }), sessionOf: () => null,
    active: () => !fence.taskMutationsPending(scope.deviceId), mutationEpoch: () => fence.taskMutationEpoch(scope.deviceId) });
  await aggregate.refresh();
};
beforeEach(mountFixture);

const disposeFixture = () => {
  pane?.dispose(); aggregate?.dispose();
  writeGate.resolve(); mutation.resolve({}); oldAnswer.resolve({});
  fence?.resetTaskMutationFences();
};
if (!tracePath) afterEach(disposeFixture);
else afterEach(async (context) => {
  try {
    traceAddresses.forEach((address) => snapshotPublication("final-read", address));
    // Reads triggered by announcements are later observations, not fabricated
    // atomic payloads for the announcing transaction.
    const snapshots = await Promise.allSettled(traceSnapshots);
    snapshots.forEach((result) => {
      if (result.status === "rejected") tracePublication({ event: "snapshot-error", message: String(result.reason) });
    });
    const { writeFile } = await import("node:fs/promises");
    await writeFile(tracePath, JSON.stringify({ test: context.task.name,
      result: context.task.result?.state, events: publicationTrace }, null, 2) + "\n", { flag: "wx" });
  } finally {
    traceStops?.forEach((stop) => stop());
    summaryTraceSpy?.mockRestore();
    disposeFixture();
  }
});

function publicationAddresses(tracker) {
  return [summaries.summaryAddress(scope, "move-task"), tracker.tasksAddress(scope.deviceId, scope.projectId),
    tracker.tasksQueryAddress(scope.deviceId, scope.projectId, { project_id: scope.projectId, state: "open" })];
}

function snapshotPublication(event, address) {
  const entry = tracePublication({ event, address });
  traceSnapshots.push(cache.readCached(address).then((record) => { entry.cached = rowTrace(record?.value); }));
}

function installPublicationTrace(tracker) {
  const addresses = publicationAddresses(tracker);
  traceAddresses = addresses;
  traceStops = addresses.map((address) => cache.subscribeCache(address, () => snapshotPublication("announcement-read", address)));
  const writeSummary = summaries.writeSummaryTask;
  summaryTraceSpy = vi.spyOn(summaries, "writeSummaryTask").mockImplementation(
    traceWriter("writeSummaryTask", writeSummary, (args) => ({ task: args[1], read: args[2] })),
  );
  const rpc = paneCall.getMockImplementation();
  paneCall.mockImplementation((...args) => {
    tracePublication({ event: "rpc", method: args[0], params: args[1] });
    return rpc(...args);
  });
  addresses.forEach((address) => snapshotPublication("before-mount-read", address));
}

const exerciseMove = async (outcome) => {
  const reading = aggregate.refresh();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(2));
  blockedStatus = "ready";
  host.querySelector('[data-column="backlog"] .task-card')
    .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  // The intent starts synchronously before noteWritten or either cache write.
  expect(fence.taskMutationsPending(scope.deviceId)).toBeGreaterThan(0);
  await vi.waitFor(() => expect(enteredWrite).toBe(true));
  oldAnswer.resolve({ project_id: scope.projectId, unfinished_open_count: 99,
    watched_unread_total: 99, agents: [], basis: "before-move" });
  expect(await reading).toBe(false);
  expect((await aggregate.read()).unfinished_open_count).toBe(7);
  expect(await aggregate.refresh()).toBe(false);
  expect(rpc).toHaveBeenCalledTimes(2);
  writeGate.resolve();
  await vi.waitFor(() => expect(host.querySelector('[data-column="ready"] .task-card')).not.toBeNull());
  await vi.waitFor(() => expect(paneCall).toHaveBeenCalledWith("tasks.update", { task_id: "move-task", status: "ready" }));
  expect(fence.taskMutationsPending(scope.deviceId)).toBeGreaterThan(0);
  if (outcome === "rollback") {
    enteredWrite = false; writeGate = deferred(); blockedStatus = "backlog";
    mutation.reject(new Error("move refused"));
    await vi.waitFor(() => expect(enteredWrite).toBe(true));
    expect(fence.taskMutationsPending(scope.deviceId)).toBeGreaterThan(0);
    writeGate.resolve();
    await vi.waitFor(() => expect(host.querySelector('[data-column="backlog"] .task-card')).not.toBeNull());
  } else mutation.resolve({});
  await vi.waitFor(() => expect(fence.taskMutationsPending(scope.deviceId)).toBe(0));
};
it.each(["ack", "rollback"])("fences the whole optimistic move intent through %s, including blocked cache writes", exerciseMove);

it("keeps mutation fencing and the mounted card intact across fresh module lifetimes", async () => {
  await exerciseMove("ack");
  disposeFixture();
  await mountFixture();
  await exerciseMove("rollback");
});

it("projects optimistic moves and rollback into shared summaries while invalidating old memberships", async () => {
  host.querySelector('[data-column="backlog"] .task-card')
    .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  await vi.waitFor(async () => expect((await summaries.readSummaryQuery(scope, {})).tasks[0].status).toBe("ready"));
  const moved = await summaries.readSummaryQuery(scope, {});
  expect(moved.stale).toBe(true);
  expect(moved.next_cursor).toBe("old-next");
  expect(moved.tasks[0].review_summary).toEqual({ mode: "snapshot", status: "open" });
  await vi.waitFor(() => expect(paneCall).toHaveBeenCalledWith("tasks.update", { task_id: "move-task", status: "ready" }));
  mutation.reject(new Error("move refused"));
  await vi.waitFor(async () => expect((await summaries.readSummaryQuery(scope, {})).tasks[0].status).toBe("backlog"));
  await vi.waitFor(() => expect(fence.taskMutationsPending(scope.deviceId)).toBe(0));
});

it("keeps the optimistic summary ready when an actual pending detail read returns backlog before rejection", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const events = [], reads = [], answers = new WeakMap();
  let sequence = 0, updatePending = false, failure, writerSpy;
  const record = (event, fields = {}) => events.push({ sequence: ++sequence, event, ...structuredClone(fields) });
  const summary = () => summaries.readSummaryQuery(scope, {});
  const fixtureRpc = paneCall.getMockImplementation();
  try {
    // Establish the real startup full publication before gating subsequent RPCs.
    // This is ordinary content provenance, not a known-zero watch authority seed.
    await vi.waitFor(async () => {
      const detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, "move-task");
      expect(detail?.task?.__task_content_cache?.body?.read_order).toBeGreaterThan(0);
      expect(detail.task.__task_content_cache.needs_full_refresh).toBe(false);
    });
    expect((await summary()).tasks[0].status).toBe("backlog");
    expect(fence.taskMutationsPending(scope.deviceId)).toBe(0);

    const writeDetail = tracker.writeTaskRecord;
    writerSpy = vi.spyOn(tracker, "writeTaskRecord").mockImplementation(async (...args) => {
      const reading = answers.get(args[3].task);
      if (reading) {
        // Record the producer's actual pre-RPC ticket; never recapture it here.
        reading.contentRead = args[4]?.contentRead;
        record("detail-writer-start", { read: reading.id, contentRead: reading.contentRead });
      }
      try {
        const result = await writeDetail(...args);
        if (reading) {
          reading.completed = true;
          record("detail-writer-complete", { read: reading.id, result });
        }
        return result;
      } catch (error) {
        if (reading) record("detail-writer-error", { read: reading.id, message: error.message });
        throw error;
      }
    });
    paneCall.mockImplementation((method, params) => {
      if (method === "tasks.update") {
        updatePending = true;
        record("update-dispatch", { params });
        return fixtureRpc(method, params).finally(() => {
          updatePending = false;
          record("update-settled");
        });
      }
      if (method !== "tasks.get") return fixtureRpc(method, params);
      const reading = { id: reads.length + 1, gate: deferred(), pendingAtDispatch: fence.taskMutationsPending(scope.deviceId) };
      reads.push(reading);
      record("detail-dispatch", { read: reading.id, pending: reading.pendingAtDispatch, params });
      // Capture the fixture server's full DTO at this actual dispatch. The
      // gate controls delivery only; the production producer owns its ticket.
      const answer = fixtureRpc(method, params).then((value) => structuredClone(value));
      return reading.gate.promise.then(async () => {
        const raw = await answer;
        answers.set(raw.task, reading);
        record("detail-delivered", { read: reading.id, status: raw.task.status });
        return raw;
      });
    });

    record("move-intent");
    host.querySelector('[data-column="backlog"] .task-card')
      .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(fence.taskMutationsPending(scope.deviceId)).toBeGreaterThan(0);
    await vi.waitFor(async () => expect((await summary()).tasks[0].status).toBe("ready"));
    await vi.waitFor(() => expect(updatePending, "producer precondition: actual update ACK remains unresolved").toBe(true));
    const initiallyMoved = await summary();
    expect(initiallyMoved.tasks[0].status).toBe("ready");
    record("initial-ready", { status: initiallyMoved.tasks[0].status, pending: fence.taskMutationsPending(scope.deviceId) });
    await vi.waitFor(() => expect(reads.some((reading) => reading.pendingAtDispatch > 0),
      "producer precondition: an actual tasks.get must dispatch during the mutation").toBe(true));
    const reading = reads.find((candidate) => candidate.pendingAtDispatch > 0);
    record("detail-resolve-intent", { read: reading.id });
    reading.gate.resolve();
    await vi.waitFor(() => expect(reading.completed,
      "producer precondition: the delivered read must reach its real public writer").toBe(true));
    expect(reading.contentRead?.readOrder).toBeGreaterThan(0);
    expect(reading.contentRead.address).toEqual(tracker.taskAddress(scope.deviceId, scope.projectId, "move-task"));
    expect(reading.contentRead.ticket.readOrder).toBe(reading.contentRead.readOrder);
    expect(updatePending).toBe(true);
    expect(fence.taskMutationsPending(scope.deviceId)).toBeGreaterThan(0);
    const beforeReject = await summary();
    record("before-reject-summary", { status: beforeReject.tasks[0].status, pending: fence.taskMutationsPending(scope.deviceId) });
    expect(beforeReject.tasks[0].status).toBe("ready");
  } catch (error) {
    failure = error;
    record("assertion-failure", { message: error.message });
  } finally {
    if (updatePending) {
      // No await separates this ordering witness from the rejection itself.
      record("reject-intent");
      mutation.reject(new Error("move refused"));
      try {
        await vi.waitFor(async () => expect((await summary()).tasks[0].status).toBe("backlog"));
        await vi.waitFor(() => expect(fence.taskMutationsPending(scope.deviceId)).toBe(0));
        record("rollback-complete", { status: (await summary()).tasks[0].status, pending: fence.taskMutationsPending(scope.deviceId) });
      } catch (error) {
        record("rollback-assertion-failure", { message: error.message });
        failure ||= error;
      }
    }
    pane.dispose();
    reads.forEach((reading) => reading.gate.resolve());
    writerSpy?.mockRestore();
    const sink = process.env.TASK891_PENDING_MOVE_TRACE;
    if (sink) {
      const { writeFile } = await import("node:fs/promises");
      await writeFile(sink, JSON.stringify({ test: "pending detail read before rejection", events }, null, 2) + "\n", { flag: "wx" });
    }
  }
  if (failure) throw failure;
});

it("projects an actual create action response compactly without claiming full query membership", async () => {
  host.querySelector("[data-task-new]").click();
  const created = task({ id: "new-task", project_id: scope.projectId, number: 2, title: "A new task", body: "large body excluded",
    attachments: [{ id: "file", name: "file" }], dependencies: { blocked: true, waiting_on_count: 1,
      waiting_on: [{ task_id: "prerequisite" }] } });
  composerOptions.onFiled({ task: created }, {});
  await vi.waitFor(async () => expect((await cache.readCached(summaries.summaryAddress(scope, created.id)))?.value?.task?.title)
    .toBe(created.title));
  const projected = (await cache.readCached(summaries.summaryAddress(scope, created.id))).value.task;
  expect(projected.attachment_count).toBe(1);
  expect(projected).not.toHaveProperty("body");
  expect(projected).not.toHaveProperty("attachments");
  expect(projected.dependencies).toEqual({ blocked: true, waiting_on_count: 1 });
  const prior = await summaries.readSummaryQuery(scope, {});
  expect(prior.ids).toEqual(["move-task"]);
  expect(prior.stale).toBe(true);
  await vi.waitFor(() => expect(fence.taskMutationsPending(scope.deviceId)).toBe(0));
});
