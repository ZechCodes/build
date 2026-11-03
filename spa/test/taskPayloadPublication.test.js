import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { comment, task } from "./trackerWireFixture.js";

const identity = { accountId: "payload-account", principalId: "payload-principal", deviceId: "payload-device" };
const scope = { deviceId: identity.deviceId, projectId: "payload-project" };
const greeting = { bridge_epoch: "payload-epoch", journal_incarnation: "payload-journal",
  capabilities: ["bridge.epoch"], ack_confirm_methods: ["tasks.update", "tasks.comment"] };
const publication = (generation = 2) => [{ kind: "project", id: scope.projectId, generation, sequence: 11 }];
let runtimes, services, gates;

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  runtimes = []; services = []; gates = [];
});

afterEach(async () => {
  for (const gate of gates) gate.resolve({});
  for (const running of services) { await running.drain(); await running.service.close(); }
  vi.restoreAllMocks();
  for (const runtime of runtimes) {
    runtime.operations.deactivateProjection();
    let db;
    await runtime.local.transactWriteOperations("readonly", ({ records }) => { db = records.transaction.db; });
    db?.close();
  }
});

const deferred = () => {
  const gate = Promise.withResolvers();
  gates.push(gate);
  return gate;
};

async function independentRuntime() {
  vi.resetModules();
  const [local, tracker, content, summaries, watch, order, keys, refresh, operationCache, operationService] = await Promise.all([
    import("../src/core/localCache.js"), import("../src/core/trackerCache.js"),
    import("../src/core/taskContentAuthority.js"), import("../src/core/taskSummaryCache.js"),
    import("../src/core/taskWatchAuthority.js"), import("../src/core/taskReadOrder.js"),
    import("../src/core/idbRecords.js"), import("../src/core/writeOperationRefresh.js"),
    import("../src/core/writeOperationCache.js"), import("../src/core/writeOperations.js"),
  ]);
  local.setCacheRecoveryTiming({ reopenDelaysMs: [0, 1, 2, 3], openTimeoutMs: 200, restMs: 40 });
  let db;
  expect((await local.transactWriteOperations("readonly", ({ records }) => { db = records.transaction.db; })).committed).toBe(true);
  for (const prior of runtimes) {
    expect(local).not.toBe(prior.local);
    expect(content).not.toBe(prior.content);
    expect(db).not.toBe(prior.db);
  }
  const operations = operationCache.createWriteOperationCache(identity);
  operations.activateProjection();
  const runtime = { local, tracker, content, summaries, watch, order, keys, refresh, operations, operationService, db };
  runtimes.push(runtime);
  return runtime;
}

async function seedTask(runtime, id = "payload-task", overrides = {}, timeline = []) {
  const row = task({ id, project_id: scope.projectId, title: "Before payload", body: "Before body", watched: true,
    read_through: "tc-10", watch_started_after: "te-05", ...overrides });
  const address = runtime.tracker.taskAddress(scope.deviceId, scope.projectId, id);
  const contentRead = await runtime.content.captureTaskContentRead(address, { revalidate: "full" });
  await runtime.tracker.writeTaskRecord(scope.deviceId, scope.projectId, id, runtime.tracker.taskRecord(row, timeline), { contentRead });
  const held = await runtime.local.readCanonicalCached(address);
  expect(held.value.task.body).toBe(row.body);
  expect(held.value.task.__task_content_cache.body.read_order).toBe(contentRead.readOrder);
  return { address, row };
}

async function seedUncertainTask(runtime, ...args) {
  const target = await seedTask(runtime, ...args);
  expect(await runtime.watch.beginTaskWatchMutation(scope, target.row.id,
    { watched: false, metadataOnly: true })).not.toBeNull();
  return target;
}

async function confirmedOperation(runtime, targets, { operationId = "payload-operation", overlay = true, generation = 2 } = {}) {
  const first = targets[0], published = publication(generation);
  const effects = overlay ? [{ address: first.address, kind: "upsert", path: ["timeline"], key: "id",
    value: comment({ id: `${first.row.id}-comment`, task_id: first.row.id, body: "Accepted draft" }) }] : [];
  await runtime.operations.put({ operationId, revision: 0, owner: { projectId: scope.projectId },
    journalIncarnation: greeting.journal_incarnation,
    request: { method: "tasks.comment", params: { task_id: first.row.id, body: "Accepted draft" } } });
  await runtime.operations.accept(operationId, { revision: 1, ack: {}, owners: published, overlay: effects });
  const frame = { type: "writes.confirmed", operation_id: operationId, bridge_epoch: greeting.bridge_epoch,
    revision: 2, state: "succeeded", published, refresh: targets.map(({ row }) => ({ kind: "task", id: row.id })) };
  const record = await runtime.operations.confirm(operationId, { revision: frame.revision, confirmation: frame, refreshPending: true });
  expect(record.refreshPending).toBe(true);
  expect(record.overlay).toHaveLength(overlay ? 1 : 0);
  // Empty normalized effects have no active layer; their named refresh
  // obligation remains independently pending.
  expect(record.overlayActive).toBe(overlay);
  return { record, frame };
}

const answerFor = ({ row }, overrides = {}) => ({
  task: { ...row, title: "Admitted payload", body: "Admitted body", updated_at: "2026-10-30T00:00:00Z", ...overrides },
  timeline: [comment({ id: `${row.id}-comment`, task_id: row.id, body: "Canonical comment" })],
});

async function capturedEntry(runtime, target, answer = answerFor(target), published = publication()) {
  const contentRead = await runtime.content.captureTaskContentRead(target.address, { revalidate: "full" });
  const detailBasis = await runtime.tracker.captureTaskDetailRead(scope.deviceId, scope.projectId, target.row.id);
  expect(contentRead.ticket).not.toBeNull();
  return { address: target.address, value: answer, publication: published, contentRead,
    guard: { written: contentRead.written, detailBasis } };
}

const operationUnfinished = async (runtime, id = "payload-operation", overlay = true) => {
  expect(await runtime.operations.get(id)).toMatchObject({ refreshPending: true, overlayActive: overlay });
};

async function snapshot(runtime, targets, operationId = "payload-operation") {
  const addresses = targets.flatMap(({ address, row }) => [address, runtime.summaries.summaryAddress(scope, row.id), runtime.order.readsAddress(address)]);
  return { records: await Promise.all(addresses.map((address) => runtime.local.readCanonicalCached(address))),
    operation: await runtime.operations.get(operationId) };
}

function serviceObservation(observation, event, value) {
  if (!observation) return;
  try { observation[event]?.(value); }
  catch (error) { observation.errors.push({ event, message: error.message }); }
}

async function startService(runtime, answer, calls = [], { waitForRefresh = true, read, observation } = {}) {
  const pending = new Set(), timers = new Map();
  let timerId = 0;
  const session = { onWriteFrame: () => () => {}, callRaw: vi.fn(async (method, params, options) => {
    calls.push({ method, params });
    options?.assertCurrent?.();
    if (method.startsWith("writes.")) return { journal_incarnation: greeting.journal_incarnation, operations: [] };
    if (read && ["tasks.get", "thread.page"].includes(method)) return read(method, params);
    if (method === "tasks.get") return answer(params.task_id);
    return { ok: true, bridge_epoch: greeting.bridge_epoch, write: { phase: "ack", operation_id: options.write.operation_id,
      revision: 1, state: "accepted", accepted_epoch: greeting.bridge_epoch }, result: { saved: true } };
  }) };
  const service = runtime.operationService.createWriteOperationService({ ...identity, cache: runtime.operations,
    setTimeout: (run, wait) => {
      const id = ++timerId;
      timers.set(id, { run: observation ? () => { serviceObservation(observation, "fired", { id }); return run(); } : run, wait });
      serviceObservation(observation, "scheduled", { id, wait, source: observation ? new Error().stack : undefined });
      return id;
    },
    clearTimeout: (id) => {
      const removed = timers.delete(id);
      serviceObservation(observation, "cancelled", { id });
      return removed;
    }, random: () => .5,
    onRefresh: (_addresses, frame, record, authority) => {
      const reading = runtime.refresh.refreshWriteOperation({ cache: runtime.operations, frame, record, authority });
      pending.add(reading);
      reading.then((result) => {
        pending.delete(reading);
        serviceObservation(observation, "refreshed", { reading, result, frame, record, authority });
      }, (error) => {
        pending.delete(reading);
        serviceObservation(observation, "refreshFailed", { error });
      });
      return reading;
    },
  });
  const drain = async () => {
    while (pending.size) await Promise.allSettled([...pending]);
    // Observe the service consuming the committed refresh state, not just the
    // inner publication callback returning before finishTaskRefresh runs.
    const expected = await runtime.operations.list();
    const consumed = () => sameValues(service.readOperations(), expected);
    if (!consumed()) await new Promise((resolve) => {
      const stop = service.subscribe(() => { if (consumed()) { stop(); resolve(); } });
      if (consumed()) { stop(); resolve(); }
    });
    await Promise.resolve();
  };
  const running = { service, session, calls, timers, drain, pending };
  services.push(running);
  serviceObservation(observation, "created", running);
  await service.bind({ session, greeting });
  if (waitForRefresh) await drain();
  return running;
}

const sameValues = (left, right) => JSON.stringify(left) === JSON.stringify(right);

function matchingBlockedSnapshot(rows, completed, expected, keyOf) {
  if (!completed || rows.length !== 1) return false;
  const row = rows[0], state = row.refreshState, refresh = completed.result?.refresh;
  return matchingSnapshotOperation(row, expected) && matchingSnapshotGeneration(row, state, refresh)
    && matchingSnapshotTargets(state, expected, keyOf);
}

function matchingSnapshotOperation(row, expected) {
  return row.namespace === expected.record.namespace && row.operationId === expected.record.operationId
    && row.revision === expected.frame.revision && row.refreshPending === true && row.overlayActive === true
    && row.state === "succeeded" && sameValues(row.confirmation, expected.frame);
}

function matchingSnapshotGeneration(row, state, refresh) {
  if (!state || !refresh) return false;
  return row.operationId === refresh.operationId && row.revision === refresh.revision
    && state.version === 1 && state.revision === row.revision && typeof refresh.generation === "string"
    && refresh.generation.length > 0 && state.generation === refresh.generation;
}

function matchingSnapshotTargets(state, expected, keyOf) {
  return sameValues(state.required, expected.frame.published)
    && sameValues(state.targets.map(({ address }) => keyOf(address)).sort(), expected.keys)
    && state.targets.every((target) => target.key === keyOf(target.address) && target.phase === "blocked" && target.receipt === null);
}

function timerBoundaryObservation(runtime, expected) {
  const events = [], errors = [], unread = Promise.withResolvers(), completed = Promise.withResolvers(), probe = Promise.withResolvers();
  let sequence = 0, currentSnapshot = null, finished = null, accepted = null, timers;
  const note = (type, fields = {}) => {
    const event = { sequence: ++sequence, type, ...fields };
    events.push(event);
    return event;
  };
  const liveTimers = () => [...timers].map(([id, { wait }]) => ({ id, wait }));
  const observation = {
    errors, events, unread, completed, probe,
    scheduled(value) { note("scheduled", value); },
    cancelled(value) { note("cancelled", { ...value, snapshot: currentSnapshot }); },
    fired(value) { note("fired", value); },
    refreshed(value) {
      finished = note("inner-completed", value);
      completed.resolve(finished);
    },
    refreshFailed({ error }) { completed.resolve(note("inner-failed", { message: error.message })); },
    created(running) {
      timers = running.timers;
      observation.stop = running.service.subscribe((rows) => {
        try {
          const event = note("snapshot", { rows: structuredClone(rows) });
          const row = rows.find(({ operationId }) => operationId === expected.record.operationId);
          if (!finished && row?.refreshState?.targets?.some(({ phase }) => phase === "unread")) unread.resolve(event);
          if (accepted || !matchingBlockedSnapshot(rows, finished, expected, runtime.keys.recordKey)) return;
          accepted = event;
          event.live = liveTimers();
          currentSnapshot = event.sequence;
          queueMicrotask(() => {
            try { probe.resolve(note("probe", { snapshot: event.sequence, live: liveTimers() })); }
            catch (error) { errors.push({ event: "probe", message: error.message }); probe.resolve(null); }
            currentSnapshot = null;
          });
        } catch (error) { errors.push({ event: "snapshot", message: error.message }); probe.resolve(null); }
      });
    },
    diagnostic() {
      return events.map(({ sequence: seq, type, id, wait, snapshot, live, rows, result, source }) => ({
        sequence: seq, type, id, wait, snapshot, live, refresh: result?.refresh,
        targets: rows?.flatMap((row) => (row.refreshState?.targets || []).map(({ key, phase }) => ({ key, phase }))),
        source: source?.split("\n").slice(0, 6),
      }));
    },
  };
  return observation;
}

it("publishes admitted task payload under unknown watch authority without a receipt or completion", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const target = await seedUncertainTask(writer), { record, frame } = await confirmedOperation(writer, [target]);
  expect(await writer.watch.readTaskWatchState(scope)).toMatchObject({ uncertain: true, eligible: false });
  const beforeBasis = await writer.tracker.captureTaskDetailRead(scope.deviceId, scope.projectId, target.row.id);
  const answer = answerFor(target);
  let issued;
  const authority = { active: () => true, call: vi.fn(async () => {
    issued = (await writer.local.readCanonicalCached(writer.order.TASK_READ_COUNT_ADDRESS)).value;
    return answer;
  }) };
  const result = await writer.refresh.refreshWriteOperation({ cache: writer.operations, frame, record, authority })
    .then((value) => ({ value }), (error) => ({ error }));
  expect(authority.call).toHaveBeenCalledExactlyOnceWith("tasks.get", { task_id: target.row.id });
  const canonical = await observer.local.readCanonicalCached(target.address);
  // First required RED is actual public payload availability, not a new API.
  expect(canonical.value.task.body).toBe(answer.task.body);
  expect(canonical.value.task.title).toBe(answer.task.title);
  expect(canonical.value.timeline).toEqual(answer.timeline);
  expect((await observer.local.readCached(target.address)).value.task.body).toBe(answer.task.body);
  expect((await observer.local.readCanonicalCached(observer.summaries.summaryAddress(scope, target.row.id))).value.task.title).toBe(answer.task.title);
  expect(canonical.value.task.__task_content_cache.header.read_order).toBe(issued);
  expect(canonical.value.task.__task_content_cache.body.read_order).toBe(issued);
  expect(await observer.tracker.captureTaskDetailRead(scope.deviceId, scope.projectId, target.row.id)).toBeGreaterThan(beforeBasis);
  expect(canonical.publication || []).toEqual([]);
  expect(await observer.watch.readTaskWatchState(scope)).toMatchObject({ uncertain: true, eligible: false });
  await operationUnfinished(observer);
  expect(result.error?.message || "").not.toContain("superseded");
});

it("keeps rejected components and protected watch paint during partial task publication", async () => {
  const writer = await independentRuntime();
  const target = await seedTask(writer, "protected-task", { updated_at: "2026-11-01T00:00:00Z",
    review_summary: { task_id: "review", mode: "pull_request", version: 1, status: "open" } });
  await confirmedOperation(writer, [target]);
  const owner = await writer.watch.beginTaskWatchMutation(scope, target.row.id, { watched: false });
  expect(owner).not.toBeNull();
  const entry = await capturedEntry(writer, target, answerFor(target, { watched: false, read_through: "tc-20",
    review_summary: { task_id: "review", mode: "pull_request", version: 2, status: "closed" } }));
  await writer.operations.observePublication({ canonical: [entry] });
  const canonical = await writer.local.readCanonicalCached(target.address);
  expect(canonical.value.task.body).toBe(target.row.body);
  expect(canonical.value.task.title).toBe(target.row.title);
  expect(canonical.value.timeline).toEqual([]);
  expect(canonical.value.task.watched).toBe(true);
  expect(canonical.value.task.read_through).toBe("tc-20");
  expect(canonical.value.task.review_summary.version).toBe(2);
  expect(canonical.publication || []).toEqual([]);
  expect(await writer.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true });
  await operationUnfinished(writer);
});

it("admits canonical history under its read fence while retaining a timestamp-protected body", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const heldComment = comment({ id: "held-history", body: "Held history" });
  const target = await seedTask(writer, "history-task", { updated_at: "2026-11-01T00:00:00Z" }, [heldComment]);
  const { frame, record } = await confirmedOperation(writer, [target]);
  const before = await writer.local.readCanonicalCached(target.address);
  const answer = answerFor(target);
  const authority = { active: () => true, call: vi.fn(async () => answer) };
  await writer.refresh.refreshWriteOperation({ cache: writer.operations, frame, record, authority }).catch(() => {});
  expect(authority.call).toHaveBeenCalledExactlyOnceWith("tasks.get", { task_id: target.row.id });
  const canonical = await observer.local.readCanonicalCached(target.address);
  expect(canonical.value.task.body).toBe(target.row.body);
  expect(canonical.value.task.title).toBe(target.row.title);
  expect(canonical.value.task.__task_content_cache.body).toEqual(before.value.task.__task_content_cache.body);
  // This full canonical history came from the actual read owner after its
  // addressed capture; body timestamp selection does not certify its order.
  expect(canonical.value.timeline).toEqual(answer.timeline);
  expect(canonical.publication || []).toEqual([]);
  await operationUnfinished(observer);
});

it("does not infer canonical history authority from an admitted ordinary body alone", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const heldComment = comment({ id: "held-history", body: "Keep this canonical history" });
  const target = await seedTask(writer, "body-only-task", {}, [heldComment]);
  await confirmedOperation(writer, [target]);
  const entry = await capturedEntry(writer, target);
  // A body-only producer omits timeline. Its ordinary ticket does not mean
  // the task's complete canonical history was returned or admitted.
  delete entry.value.timeline;
  await writer.operations.observePublication({ canonical: [entry] });
  const canonical = await observer.local.readCanonicalCached(target.address);
  expect(canonical.value.task.body).toBe(entry.value.task.body);
  expect(canonical.value.timeline).toEqual([heldComment]);
  expect(canonical.publication || []).toEqual([]);
  await operationUnfinished(observer);
});

it("distinguishes a changed written or detail fence from missing watch authority", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const target = await seedTask(writer);
  await confirmedOperation(writer, [target]);
  const entry = await capturedEntry(writer, target);
  for (const body of ["Optimistic body", target.row.body]) await writer.content.mergeTaskContentRecordsTogether(scope,
    [target.address], ([held]) => ({ values: [{ ...held.value,
      task: writer.content.retireTaskContent({ ...held.value.task, body }, "body") }], retire: true }));
  const before = await snapshot(observer, [target]);
  expect((await writer.local.readCanonicalCached(target.address)).write).not.toBe(entry.guard.written);
  expect(await writer.operations.observePublication({ canonical: [entry] })).toEqual([]);
  expect(await snapshot(observer, [target])).toEqual(before);
  await operationUnfinished(observer);
});

it("rolls payload and its blocked refresh outcome back together on quota failure", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const target = await seedTask(writer);
  await confirmedOperation(writer, [target]);
  const entry = await capturedEntry(writer, target), before = await snapshot(observer, [target]);
  const detailKey = writer.keys.recordKey(target.address), summaryKey = writer.keys.recordKey(writer.summaries.summaryAddress(scope, target.row.id));
  const originalPut = IDBObjectStore.prototype.put;
  let detailAttempted = false, refused = false;
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (key === detailKey && record.value?.task?.title === entry.value.task.title) detailAttempted = true;
    if (detailAttempted && key === summaryKey) {
      refused = true;
      throw new DOMException("Quota while committing partial task publication", "QuotaExceededError");
    }
    return originalPut.call(this, record, key);
  });
  const heard = [];
  const stop = writer.local.subscribeCache({ deviceId: scope.deviceId, entityId: scope.projectId }, (address) => heard.push(address));
  const result = await writer.operations.observePublication({ canonical: [entry] }).then(() => null, (error) => error);
  stop();
  expect(detailAttempted).toBe(true);
  expect(refused).toBe(true);
  expect(result?.name).toBe("QuotaExceededError");
  expect(await snapshot(observer, [target])).toEqual(before);
  expect(heard).toEqual([]);
});

it("does not reuse an aborted publication outcome after session retirement", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const target = await seedTask(writer);
  await confirmedOperation(writer, [target]);
  const entry = await capturedEntry(writer, target), before = await snapshot(observer, [target]);
  const originalPut = IDBObjectStore.prototype.put, keyOfDetail = writer.keys.recordKey(target.address);
  let active = true, attempted = 0;
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (key === keyOfDetail && record.value?.task?.title === entry.value.task.title) {
      attempted += 1;
      if (attempted === 1) { active = false; throw new DOMException("Connection closed", "InvalidStateError"); }
    }
    return originalPut.call(this, record, key);
  });
  const written = await writer.operations.observePublication({ canonical: [entry], accept: () => active });
  expect(attempted).toBe(1);
  expect(written).toEqual([]);
  expect(await snapshot(observer, [target])).toEqual(before);
});

it("cannot park a newer refresh generation or replacement session with an older response", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const target = await seedTask(writer), { frame, record } = await confirmedOperation(writer, [target]);
  const reply = deferred();
  let active = true;
  const authority = { active: () => active, call: vi.fn(() => reply.promise) };
  const reading = writer.refresh.refreshWriteOperation({ cache: writer.operations, frame, record, authority }).catch((error) => error);
  await vi.waitFor(() => expect(authority.call).toHaveBeenCalledTimes(1));
  const next = { ...frame, revision: 3, published: publication(3) };
  await writer.operations.confirm(record.operationId, { revision: next.revision, confirmation: next, refreshPending: true });
  const before = await snapshot(observer, [target]);
  active = false;
  reply.resolve(answerFor(target));
  expect((await reading).message).toContain("opening");
  expect(await snapshot(observer, [target])).toEqual(before);
  expect(await observer.operations.get(record.operationId)).toMatchObject({ revision: 3, refreshPending: true, overlayActive: true });
});

it("cannot park a newer confirmed obligation while its original session remains current", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const target = await seedTask(writer), laterTarget = await seedTask(writer, "later-target");
  const { frame, record } = await confirmedOperation(writer, [target]);
  const reply = deferred();
  const authority = { active: () => true, call: vi.fn(() => reply.promise) };
  const reading = writer.refresh.refreshWriteOperation({ cache: writer.operations, frame, record, authority }).catch((error) => error);
  await vi.waitFor(() => expect(authority.call).toHaveBeenCalledTimes(1));
  const later = { ...frame, revision: 3, published: publication(3), refresh: [{ kind: "task", id: laterTarget.row.id }] };
  await writer.operations.confirm(record.operationId, { revision: 3, confirmation: later, refreshPending: true });
  const before = await observer.operations.get(record.operationId);
  reply.resolve(answerFor(target));
  await reading;
  expect(authority.active()).toBe(true);
  expect(await observer.operations.get(record.operationId)).toEqual(before);
  expect((await observer.local.readCanonicalCached(laterTarget.address)).value.task.body).toBe(laterTarget.row.body);
});

it("retains a blocked refresh across reopen without retrying or blocking a new user write", async () => {
  const writer = await independentRuntime(), target = await seedUncertainTask(writer);
  await confirmedOperation(writer, [target]);
  const calls = [], running = await startService(writer, () => answerFor(target), calls);
  expect(calls.filter(({ method }) => method === "tasks.get")).toHaveLength(1);
  expect(running.timers.size).toBe(0);
  await running.service.reconcile(); await running.drain();
  expect(calls.filter(({ method }) => method === "tasks.get")).toHaveLength(1);
  await running.service.bind({ session: running.session, greeting }); await running.drain();
  expect(calls.filter(({ method }) => method === "tasks.get")).toHaveLength(1);
  expect(running.timers.size).toBe(0);
  await operationUnfinished(writer);
  await expect(running.service.submit({ operationId: "independent-write", method: "tasks.update",
    params: { task_id: "other-task", title: "User edit" } })).resolves.toEqual({ saved: true });
  expect(calls.some(({ method, params }) => method === "tasks.update" && params.task_id === "other-task")).toBe(true);
  await running.service.close();
  const reopened = await independentRuntime();
  const reopenedService = await startService(reopened, () => answerFor(target), calls);
  await reopenedService.service.reconcile(); await reopenedService.drain();
  expect(calls.filter(({ method }) => method === "tasks.get")).toHaveLength(1);
  await operationUnfinished(reopened);
});

it("does not retain a retry timer after the blocked refresh is observed by the service", async () => {
  const writer = await independentRuntime(), target = await seedUncertainTask(writer);
  const { record, frame } = await confirmedOperation(writer, [target]);
  const expected = { record, frame, keys: [writer.keys.recordKey(target.address)] };
  const observation = timerBoundaryObservation(writer, expected);
  const reply = deferred(), dispatched = Promise.withResolvers(), calls = [];
  const answer = answerFor(target);
  try {
    const running = await startService(writer, () => answer, calls, { waitForRefresh: false, observation,
      read: (method, params) => { dispatched.resolve({ method, params }); return reply.promise; } });
    expect(await dispatched.promise).toEqual({ method: "tasks.get", params: { task_id: target.row.id } });
    const earlier = await observation.unread.promise;
    expect(earlier.rows).toHaveLength(1);
    expect(earlier.rows[0]).toMatchObject({ operationId: record.operationId, revision: frame.revision,
      refreshPending: true, refreshState: { targets: [{ key: expected.keys[0], phase: "unread" }] } });
    // bind has completed its lease; a real retry token, not an inserted timer,
    // must exist before the genuine reply is released.
    expect(running.timers.size).toBe(1);
    reply.resolve(answer);
    const completed = await observation.completed.promise;
    expect(completed.type).toBe("inner-completed");
    expect(completed.authority.active()).toBe(true);
    expect(completed.result).toMatchObject({ complete: false, refresh: { operationId: record.operationId,
      revision: frame.revision, generation: expect.any(String), state: "blocked" },
    outcomes: [{ address: target.address, payloadWritten: true, historyWritten: true,
      receiptPublished: false, reason: "missing-complete-authority" }] });
    expect(completed.result.refresh.generation).not.toBe("");
    expect(earlier.rows[0].refreshState.generation).toBe(completed.result.refresh.generation);
    expect(completed.result.outcomes.map(({ address }) => writer.keys.recordKey(address)).sort()).toEqual(expected.keys);
    const canonical = await writer.local.readCanonicalCached(target.address);
    expect(canonical.value.task.body).toBe(answer.task.body);
    expect(canonical.publication || []).toEqual([]);
    const checkpoint = await observation.probe.promise;
    expect(observation.errors).toEqual([]);
    expect(checkpoint).not.toBeNull();
    const accepted = observation.events.find(({ sequence }) => sequence === checkpoint.snapshot);
    expect(earlier.sequence).toBeLessThan(completed.sequence);
    expect(completed.sequence).toBeLessThan(accepted.sequence);
    expect(accepted.rows).toHaveLength(1);
    expect(accepted.rows[0]).toMatchObject({ namespace: record.namespace, operationId: record.operationId,
      revision: frame.revision, state: "succeeded", refreshPending: true, overlayActive: true,
      refreshState: { revision: frame.revision, generation: completed.result.refresh.generation,
        required: frame.published, targets: [{ key: expected.keys[0], address: target.address, phase: "blocked", receipt: null }] } });
    expect(accepted.rows[0].refreshState.targets.map(({ key }) => key).sort()).toEqual(expected.keys);
    expect(accepted.rows[0].confirmation).toEqual(frame);
    // Without an actually live token this is a failed precondition, never a
    // zero-timer GREEN that could survive removal of worker.reset.
    expect(accepted.live).toHaveLength(1);
    const token = accepted.live[0];
    const registration = observation.events.find(({ type, id }) => type === "scheduled" && id === token.id);
    expect(registration.wait).toBe(token.wait);
    expect(registration.sequence).toBeLessThan(accepted.sequence);
    const cancellations = observation.events.filter(({ type, id, snapshot }) => type === "cancelled"
      && id === token.id && snapshot === accepted.sequence);
    expect(cancellations).toHaveLength(1);
    expect(cancellations[0].sequence).toBeGreaterThan(accepted.sequence);
    expect(cancellations[0].sequence).toBeLessThan(checkpoint.sequence);
    expect(checkpoint.live).toEqual([]);
    expect(running.timers.size).toBe(0);
    expect(observation.events.filter(({ type }) => type === "fired")).toEqual([]);
    expect(calls).toEqual([{ method: "tasks.get", params: { task_id: target.row.id } }]);
    expect((await writer.local.readCanonicalCached(target.address)).publication || []).toEqual([]);
    await operationUnfinished(writer);
  } finally {
    observation.stop?.();
    reply.resolve(answer);
    console.info("D3 timer completion trace", JSON.stringify(observation.diagnostic()));
  }
});

it("does not wake a blocked obligation for unrelated or still unknown cache evidence", async () => {
  const writer = await independentRuntime(), target = await seedUncertainTask(writer);
  await confirmedOperation(writer, [target]);
  const calls = [], running = await startService(writer, () => answerFor(target), calls);
  await writer.local.writeCached({ deviceId: scope.deviceId, entityId: scope.projectId, kind: "unrelated" }, { value: 1 });
  const prior = await writer.local.readCanonicalCached(target.address);
  await writer.local.writeCached(target.address, prior.value, { publication: publication(1) });
  const otherOwner = await writer.watch.beginTaskWatchMutation(scope, "other-task", { watched: true, metadataOnly: true });
  expect(otherOwner).not.toBeNull();
  await writer.watch.settleTaskWatchMutation(otherOwner, { rollback: true });
  expect(await writer.watch.readTaskWatchState(scope)).toMatchObject({ uncertain: true, eligible: false });
  await running.service.checkAgain("payload-operation"); await running.drain();
  expect(calls.filter(({ method }) => method === "tasks.get")).toHaveLength(1);
  await operationUnfinished(writer);
  // The genuine pending owner keeps this obligation blocked.
});

it.each(["fresh", "reset", "orphan"])("does not turn %s ordinary proof or an empty overlay into a full task receipt", async (kind) => {
  const writer = await independentRuntime();
  let target = await seedTask(writer);
  const { record } = await confirmedOperation(writer, [target], { overlay: false });
  if (kind !== "fresh") {
    expect(await writer.watch.beginTaskWatchMutation(scope, target.row.id, { watched: false, metadataOnly: true })).not.toBeNull();
    if (kind === "reset") { await writer.local.wipeCache(); target = await seedTask(writer); }
  }
  const entry = await capturedEntry(writer, target);
  expect(entry.contentRead.ticket.watchUncertain).toBe(kind === "orphan");
  await writer.operations.observePublication({ canonical: [entry] });
  const held = await writer.local.readCanonicalCached(target.address);
  expect(held.value.task.body).toBe(entry.value.task.body);
  expect(held.publication || []).toEqual([]);
  await operationUnfinished(writer, "payload-operation", record.overlayActive);
  expect(await writer.watch.readTaskWatchState(scope)).toMatchObject({ uncertain: kind === "orphan", eligible: kind !== "orphan" });
});

it("keeps multi-target completion pending across generation changes and reopen", async () => {
  const writer = await independentRuntime();
  const first = await seedTask(writer, "first-task"), second = await seedTask(writer, "second-task");
  const { frame, record } = await confirmedOperation(writer, [first, second]);
  // Preexisting disk vectors are adversarial historical artifacts, not a
  // genuine authority producer for the new obligation's generation.
  for (const target of [first, second]) {
    const prior = await writer.local.readCanonicalCached(target.address);
    await writer.local.writeCached(target.address, prior.value, { publication: publication(1) });
  }
  const entry = await capturedEntry(writer, first);
  await writer.operations.observePublication({ canonical: [entry] });
  expect((await writer.local.readCanonicalCached(first.address)).value.task.body).toBe(entry.value.task.body);
  expect((await writer.local.readCanonicalCached(first.address)).publication || []).toEqual([]);
  await operationUnfinished(writer);
  const later = { ...frame, revision: 3, published: publication(3) };
  await writer.operations.confirm(record.operationId, { revision: 3, confirmation: later, refreshPending: true });
  const reopened = await independentRuntime(), before = await reopened.operations.get(record.operationId);
  expect(before.confirmation.refresh).toHaveLength(2);
  const oldGeneration = await capturedEntry(reopened, second, answerFor(second), publication(2));
  await reopened.operations.observePublication({ canonical: [oldGeneration] });
  expect((await reopened.local.readCanonicalCached(second.address)).publication || []).toEqual([]);
  expect(await reopened.operations.get(record.operationId)).toEqual(before);
  expect(await reopened.operations.get(record.operationId)).toMatchObject({ revision: 3, refreshPending: true, overlayActive: true });
});

it("rolls every named target back when the second payload put aborts", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const first = await seedTask(writer, "first-atomic-task"), second = await seedTask(writer, "second-atomic-task");
  await confirmedOperation(writer, [first, second]);
  const entries = [await capturedEntry(writer, first), await capturedEntry(writer, second)];
  const before = await snapshot(observer, [first, second]);
  const originalPut = IDBObjectStore.prototype.put;
  const firstKey = writer.keys.recordKey(first.address), secondKey = writer.keys.recordKey(second.address);
  let firstAttempted = false, secondRefused = false;
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (key === firstKey && record.value?.task?.title === entries[0].value.task.title) firstAttempted = true;
    if (key === secondKey && record.value?.task?.title === entries[1].value.task.title) {
      secondRefused = true;
      throw new DOMException("Second target quota failure", "QuotaExceededError");
    }
    return originalPut.call(this, record, key);
  });
  const result = await writer.operations.observePublication({ canonical: entries }).then(() => null, (error) => error);
  expect(firstAttempted).toBe(true);
  expect(secondRefused).toBe(true);
  expect(result?.name).toBe("QuotaExceededError");
  expect(await snapshot(observer, [first, second])).toEqual(before);
});

it("clears an inherited vector on partial bytes without incidental layer retirement", async () => {
  const writer = await independentRuntime(), target = await seedTask(writer);
  await confirmedOperation(writer, [target]);
  const prior = await writer.local.readCanonicalCached(target.address);
  await writer.local.writeCached(target.address, prior.value, { publication: publication() });
  expect((await writer.operations.get("payload-operation")).overlayActive).toBe(true);
  const entry = await capturedEntry(writer, target);
  await writer.operations.observePublication({ canonical: [entry] });
  const after = await writer.local.readCanonicalCached(target.address);
  expect(after.value.task.body).toBe(entry.value.task.body);
  expect(after.publication || []).toEqual([]);
  await operationUnfinished(writer);
});

it("does not retire another operation from a preexisting vector during a partial publication", async () => {
  const writer = await independentRuntime(), target = await seedTask(writer), unrelated = await seedTask(writer, "other-target");
  await confirmedOperation(writer, [target]);
  await confirmedOperation(writer, [unrelated], { operationId: "unrelated-operation" });
  const prior = await writer.local.readCanonicalCached(unrelated.address);
  await writer.local.writeCached(unrelated.address, prior.value, { publication: publication() });
  expect((await writer.operations.get("unrelated-operation")).overlayActive).toBe(true);
  await writer.operations.observePublication({ canonical: [await capturedEntry(writer, target)] });
  expect((await writer.operations.get("unrelated-operation")).overlayActive).toBe(true);
  await operationUnfinished(writer);
});

it("aborts queued partial task publication when its accept guard retires during the detail put", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const target = await seedUncertainTask(writer);
  await confirmedOperation(writer, [target]);
  const entry = await capturedEntry(writer, target), before = await snapshot(observer, [target]);
  expect(await writer.watch.readTaskWatchState(scope)).toMatchObject({ uncertain: true, eligible: false });
  const originalPut = IDBObjectStore.prototype.put, detailKey = writer.keys.recordKey(target.address);
  let active = true, detailQueued = false;
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    const request = originalPut.call(this, record, key);
    if (key === detailKey && record.value?.task?.body === entry.value.task.body) {
      detailQueued = true;
      active = false;
    }
    return request;
  });
  const heard = [];
  const stop = writer.local.subscribeCache({ deviceId: scope.deviceId, entityId: scope.projectId }, (address) => heard.push(address));
  const result = await writer.operations.observePublication({ canonical: [entry], report: true, accept: () => active })
    .then((value) => ({ value }), (error) => ({ error }));
  stop();
  expect(detailQueued).toBe(true);
  expect(active).toBe(false);
  // The real put was queued before authority ended. Returning from a callback
  // cannot make that write disappear: the actual transaction must abort.
  expect(await snapshot(observer, [target])).toEqual(before);
  expect(heard).toEqual([]);
  expect(result.value?.written || []).toEqual([]);
  expect((result.value?.outcomes || []).some((outcome) => outcome.payloadWritten || outcome.receiptPublished)).toBe(false);
});

it("commits partial task publication while its accept guard remains current", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const target = await seedUncertainTask(writer);
  await confirmedOperation(writer, [target]);
  const entry = await capturedEntry(writer, target);
  const heard = [];
  const stop = writer.local.subscribeCache({ deviceId: scope.deviceId, entityId: scope.projectId }, (address) => heard.push(address));
  const result = await writer.operations.observePublication({ canonical: [entry], report: true, accept: () => true });
  stop();
  const canonical = await observer.local.readCanonicalCached(target.address);
  expect(canonical.value.task.body).toBe(entry.value.task.body);
  expect(canonical.value.task.title).toBe(entry.value.task.title);
  expect(canonical.value.timeline).toEqual([]);
  expect(canonical.publication || []).toEqual([]);
  expect(result.written).toContainEqual(target.address);
  expect(result.outcomes).toContainEqual(expect.objectContaining({ address: target.address, payloadWritten: true, receiptPublished: false }));
  expect(heard).toContainEqual(target.address);
  await operationUnfinished(observer);
});

it.each(["copied", "serialized", "foreign-runtime"])("refuses canonical history provenance from a %s refresh entry", async (kind) => {
  const writer = await independentRuntime(), foreign = kind === "foreign-runtime" ? await independentRuntime() : writer;
  const heldComment = comment({ id: "brand-held-history", body: "Retain independently protected history" });
  const target = await seedTask(writer, "brand-task", {}, [heldComment]);
  const { frame, record } = await confirmedOperation(writer, [target]);
  const answer = answerFor(target);
  let forwarded, publicationResult;
  const cache = { get: writer.operations.get, openRefresh: foreign.operations.openRefresh, observePublication: async (input) => {
    const original = input.canonical[0];
    expect(original.contentRead.ticket).not.toBeNull();
    if (kind === "copied") forwarded = { ...original };
    else if (kind === "serialized") {
      forwarded = JSON.parse(JSON.stringify(original));
      // Preserve the actual ordinary ticket, not history-owner identity. An
      // ordinary token copied onto a DTO cannot certify its history array.
      forwarded.contentRead = original.contentRead;
    } else forwarded = original;
    if (kind !== "foreign-runtime") expect(forwarded).not.toBe(original);
    publicationResult = await foreign.operations.observePublication({ ...input, canonical: [forwarded] });
    return publicationResult;
  } };
  const authority = { active: () => true, call: vi.fn(async () => answer) };
  await writer.refresh.refreshWriteOperation({ cache, frame, record, authority }).catch(() => {});
  expect(authority.call).toHaveBeenCalledExactlyOnceWith("tasks.get", { task_id: target.row.id });
  expect(forwarded).toBeDefined();
  // The real consuming Cache must admit its own permit and reach task/history
  // selection. A foreign-instance permit rejection would be a false positive.
  expect(publicationResult?.refresh?.state).toBe("blocked");
  expect(publicationResult.outcomes).toContainEqual(expect.objectContaining({ address: target.address,
    payloadWritten: true, historyWritten: false, receiptPublished: false, reason: "missing-complete-authority" }));
  const canonical = await writer.local.readCanonicalCached(target.address);
  expect(canonical.value.timeline).toEqual([heldComment]);
  expect(canonical.publication || []).toEqual([]);
  // Foreign ordinary tickets may independently fail admission. The same-owner
  // copy controls keep the genuine ordinary ticket and prove that distinction.
  if (kind !== "foreign-runtime") expect(canonical.value.task.body).toBe(answer.task.body);
  await operationUnfinished(writer);
});

function ownedReading(runtime, { frame, record }, call, active = () => true) {
  const authority = { active, call: vi.fn(call) };
  const done = runtime.refresh.refreshWriteOperation({ cache: runtime.operations, frame, record, authority })
    .then((value) => ({ value }), (error) => ({ error }));
  return { authority, done };
}

async function blockedOwnedOperation(runtime, targets, { frame, record }) {
  const saved = await runtime.operations.get(record.operationId);
  expect(saved).toMatchObject({ revision: frame.revision, refreshPending: true, overlayActive: true,
    refreshState: { version: 1, revision: frame.revision, generation: expect.any(String), required: frame.published } });
  expect(saved.refreshState.targets).toHaveLength(targets.length);
  for (const target of targets) {
    const slot = saved.refreshState.targets.find(({ address }) => runtime.keys.recordKey(address) === runtime.keys.recordKey(target.address));
    expect(slot).toMatchObject({ key: expect.any(String), address: target.address, phase: "blocked", receipt: null,
      blocked: { reason: expect.any(String), seen: { records: expect.any(Array) } } });
    expect(slot.blocked.seen.records.length).toBeGreaterThan(0);
    for (const reference of slot.blocked.seen.records) {
      expect(reference.written).toBe(runtime.local.recordWriteOf(await runtime.local.readCanonicalCached(reference.address)));
    }
  }
  return saved;
}

it("persists an exact owned partial outcome together with admitted task payload", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime(), target = await seedUncertainTask(writer);
  const confirmed = await confirmedOperation(writer, [target]), answer = answerFor(target);
  const reading = ownedReading(writer, confirmed, async () => answer);
  await reading.done;
  expect(reading.authority.call).toHaveBeenCalledExactlyOnceWith("tasks.get", { task_id: target.row.id });
  const canonical = await observer.local.readCanonicalCached(target.address);
  expect(canonical.value.task.body).toBe(answer.task.body);
  expect(canonical.value.timeline).toEqual(answer.timeline);
  expect(canonical.publication || []).toEqual([]);
  await operationUnfinished(observer);
  // First prove useful real cache paint; the missing durable outcome is then a
  // lifecycle assertion, never an invocation of an unimplemented method.
  await blockedOwnedOperation(observer, [target], confirmed);
});

it("rolls an owned partial outcome and payload back when the summary put aborts", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime(), target = await seedUncertainTask(writer);
  const confirmed = await confirmedOperation(writer, [target]), answer = answerFor(target);
  const detailKey = writer.keys.recordKey(target.address), summaryKey = writer.keys.recordKey(writer.summaries.summaryAddress(scope, target.row.id));
  const originalPut = IDBObjectStore.prototype.put;
  let dispatched, detailQueued = false, summaryRefused = false;
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (key === summaryKey && detailQueued) {
      summaryRefused = true;
      throw new DOMException("Owned partial outcome quota", "QuotaExceededError");
    }
    const request = originalPut.call(this, record, key);
    if (key === detailKey && record.value?.task?.body === answer.task.body) detailQueued = true;
    return request;
  });
  const reading = ownedReading(writer, confirmed, async () => { dispatched = await snapshot(observer, [target]); return answer; });
  const result = await reading.done;
  expect(detailQueued).toBe(true);
  expect(summaryRefused).toBe(true);
  expect(result.error?.name).toBe("QuotaExceededError");
  expect(await snapshot(observer, [target])).toEqual(dispatched);
  // The baseline is captured after the owner's open/dispatch, not before its
  // legitimate attempt registration. Both its state and payload must roll back.
  expect(dispatched.operation).toMatchObject({ refreshPending: true, overlayActive: true,
    refreshState: { revision: confirmed.frame.revision, generation: expect.any(String),
      targets: [expect.objectContaining({ address: target.address, phase: "unread", attemptId: expect.any(String) })] } });
});

it.each(["current", "retired"])("recomputes an owned partial outcome after transient abort with a %s session", async (kind) => {
  const writer = await independentRuntime(), observer = await independentRuntime(), target = await seedUncertainTask(writer);
  const confirmed = await confirmedOperation(writer, [target]), answer = answerFor(target);
  const originalPut = IDBObjectStore.prototype.put, detailKey = writer.keys.recordKey(target.address);
  let active = true, attempted = 0, dispatched, stop = () => {};
  const heard = [];
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (key === detailKey && record.value?.task?.body === answer.task.body) {
      attempted += 1;
      if (attempted === 1) {
        if (kind === "retired") active = false;
        throw new DOMException("Connection closed during owned publication", "InvalidStateError");
      }
    }
    return originalPut.call(this, record, key);
  });
  const reading = ownedReading(writer, confirmed, async () => {
    dispatched = await snapshot(observer, [target]);
    stop = writer.local.subscribeCache({ deviceId: scope.deviceId, entityId: scope.projectId }, (address) => heard.push(address));
    return answer;
  }, () => active);
  const result = await reading.done;
  stop();
  expect(reading.authority.call).toHaveBeenCalledExactlyOnceWith("tasks.get", { task_id: target.row.id });
  if (kind === "retired") {
    expect(attempted).toBe(1);
    expect(await snapshot(observer, [target])).toEqual(dispatched);
    expect(heard).toEqual([]);
    expect(result.value?.complete).not.toBe(true);
  } else {
    expect(attempted).toBe(2);
    const canonical = await observer.local.readCanonicalCached(target.address);
    expect(canonical.value.task.body).toBe(answer.task.body);
    expect(canonical.value.timeline).toEqual(answer.timeline);
    expect(canonical.publication || []).toEqual([]);
    expect(heard).toContainEqual(target.address);
    await blockedOwnedOperation(observer, [target], confirmed);
  }
});

it("rejects an older same-generation refresh attempt before the newer response writes", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime(), target = await seedUncertainTask(writer);
  const confirmed = await confirmedOperation(writer, [target]), firstReply = deferred(), nextReply = deferred();
  const active = () => true;
  const first = ownedReading(writer, confirmed, () => firstReply.promise, active);
  await vi.waitFor(() => expect(first.authority.call).toHaveBeenCalledTimes(1));
  const next = ownedReading(writer, confirmed, () => nextReply.promise, active);
  await vi.waitFor(() => expect(next.authority.call).toHaveBeenCalledTimes(1));
  const atNewDispatch = await snapshot(observer, [target]);
  const oldAnswer = answerFor(target, { title: "Old attempt", body: "Old attempt body" }), nextAnswer = answerFor(target);
  try {
    firstReply.resolve(oldAnswer);
    await first.done;
    // Neither written/detail fence has changed from a newer response yet.
    expect((await observer.local.readCanonicalCached(target.address)).value.task.body).toBe(target.row.body);
    expect(await snapshot(observer, [target])).toEqual(atNewDispatch);
  } finally {
    firstReply.resolve(oldAnswer); nextReply.resolve(nextAnswer);
    await Promise.all([first.done, next.done]);
  }
  expect((await observer.local.readCanonicalCached(target.address)).value.task.body).toBe(nextAnswer.task.body);
  await blockedOwnedOperation(observer, [target], confirmed);
});

it("fences a real replaced service opening from parking its current refresh", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime(), target = await seedUncertainTask(writer);
  const confirmed = await confirmedOperation(writer, [target]), oldReply = deferred(), nextReply = deferred();
  let reads = 0;
  const calls = [], running = await startService(writer, () => ++reads === 1 ? oldReply.promise : nextReply.promise, calls, { waitForRefresh: false });
  await vi.waitFor(() => expect(calls.filter(({ method }) => method === "tasks.get")).toHaveLength(1));
  const oldRead = [...running.pending][0];
  expect(oldRead).toBeDefined();
  const replacement = { ...running.session };
  await running.service.bind({ session: replacement, greeting });
  await vi.waitFor(() => expect(calls.filter(({ method }) => method === "tasks.get")).toHaveLength(2));
  const atNewDispatch = await snapshot(observer, [target]);
  const oldAnswer = answerFor(target, { body: "Retired opening body" }), nextAnswer = answerFor(target);
  try {
    oldReply.resolve(oldAnswer);
    await Promise.allSettled([oldRead]);
    expect((await observer.local.readCanonicalCached(target.address)).value.task.body).toBe(target.row.body);
    expect(await snapshot(observer, [target])).toEqual(atNewDispatch);
  } finally {
    oldReply.resolve(oldAnswer); nextReply.resolve(nextAnswer);
    await running.drain();
  }
  expect((await observer.local.readCanonicalCached(target.address)).value.task.body).toBe(nextAnswer.task.body);
  await blockedOwnedOperation(observer, [target], confirmed);
});

it("dispatches an independent user write without reviving parked task refresh work", async () => {
  const writer = await independentRuntime(), target = await seedUncertainTask(writer);
  await confirmedOperation(writer, [target]);
  const calls = [], running = await startService(writer, () => answerFor(target), calls);
  expect(calls.filter(({ method }) => method === "tasks.get")).toHaveLength(1);
  const result = await running.service.submit({ operationId: "independent-parked-write", method: "tasks.update",
    params: { task_id: "other-task", title: "Still dispatches" } });
  await running.drain();
  // The worker must continue genuine user work; globally disabling or clearing
  // runnable work to remove a refresh retry cannot satisfy this control.
  expect(result).toEqual({ saved: true });
  expect(calls.filter(({ method, params }) => method === "tasks.update" && params.task_id === "other-task")).toHaveLength(1);
  expect(await writer.operations.get("independent-parked-write")).toMatchObject({ acknowledged: true, state: "accepted" });
  await operationUnfinished(writer);
  expect(calls.filter(({ method }) => method === "tasks.get")).toHaveLength(1);
});

it("parks every owned target once and preserves its obligation across an actual reopen", async () => {
  const writer = await independentRuntime(), first = await seedUncertainTask(writer, "owned-first"), second = await seedUncertainTask(writer, "owned-second");
  const targets = [first, second], confirmed = await confirmedOperation(writer, targets);
  const reading = ownedReading(writer, confirmed, async (_method, { task_id }) => answerFor(targets.find(({ row }) => row.id === task_id)));
  await reading.done;
  expect(reading.authority.call.mock.calls.map(([, { task_id }]) => task_id).sort()).toEqual(targets.map(({ row }) => row.id).sort());
  for (const target of targets) {
    const canonical = await writer.local.readCanonicalCached(target.address);
    expect(canonical.value.task.body).toBe(answerFor(target).task.body);
    expect(canonical.publication || []).toEqual([]);
  }
  const before = await blockedOwnedOperation(writer, targets, confirmed);
  const reopened = await independentRuntime();
  expect(await reopened.operations.get(confirmed.record.operationId)).toEqual(before);
  const replay = ownedReading(reopened, { frame: before.confirmation, record: before }, async () => { throw new Error("Blocked reopen must not dispatch"); });
  await replay.done;
  expect(replay.authority.call).not.toHaveBeenCalled();
  expect(await reopened.operations.get(confirmed.record.operationId)).toEqual(before);
  await operationUnfinished(reopened);
});

it("rejects old owned multi-target responses after a newer confirmation is admitted", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const first = await seedUncertainTask(writer, "old-owned-target"), second = await seedUncertainTask(writer, "new-owned-target");
  const old = await confirmedOperation(writer, [first, second]), oldReply = deferred(), nextReply = deferred();
  const stale = ownedReading(writer, old, () => oldReply.promise);
  await vi.waitFor(() => expect(stale.authority.call).toHaveBeenCalledTimes(1));
  const frame = { ...old.frame, revision: 3, published: publication(3) };
  const record = await writer.operations.confirm(old.record.operationId, { revision: frame.revision, confirmation: frame, refreshPending: true });
  const current = ownedReading(writer, { frame, record }, (_method, { task_id }) => nextReply.promise.then(() =>
    answerFor([first, second].find(({ row }) => row.id === task_id))));
  await vi.waitFor(() => expect(current.authority.call).toHaveBeenCalledTimes(1));
  const before = await snapshot(observer, [first, second]);
  try {
    oldReply.resolve(answerFor(first, { body: "Superseded obligation body" }));
    await stale.done;
    expect((await observer.local.readCanonicalCached(first.address)).value.task.body).toBe(first.row.body);
    expect(await snapshot(observer, [first, second])).toEqual(before);
  } finally {
    oldReply.resolve(answerFor(first)); nextReply.resolve({});
    await Promise.all([stale.done, current.done]);
  }
  expect((await observer.local.readCanonicalCached(second.address)).value.task.body).toBe(answerFor(second).task.body);
  await blockedOwnedOperation(observer, [first, second], { frame, record });
});

async function confirmedThreadOperation(runtime, { operationId, taskTarget, taskFirst = true }) {
  const { threadCacheAddress } = await import("../src/core/conversationCache.js");
  const { normalizeIntentEffects } = await import("../src/core/writeOperationEffects.js");
  const params = { entity_id: "payload-workspace", conversation_id: "payload-conversation",
    thread_id: "payload-thread", thread_generation_revision: 3, body: "Accepted thread draft" };
  const address = threadCacheAddress({ deviceId: scope.deviceId, entityId: params.entity_id,
    conversationId: params.conversation_id, threadId: params.thread_id });
  const oldItem = { type: "message", data: { sequence: 1, role: "user", body: "Held thread message" } };
  await runtime.local.writeCached(address, { items: [oldItem], deliveredSequence: 1,
    olderItemsRemain: false, knownTotalItems: 1, activityDigests: [],
    thread_id: params.thread_id, thread_generation_revision: params.thread_generation_revision });
  const intent = { operationId, revision: 0, owner: { projectId: scope.projectId },
    journalIncarnation: greeting.journal_incarnation, request: { method: "thread.post", params } };
  const { overlay } = await normalizeIntentEffects({ ...intent, deviceId: scope.deviceId });
  expect(overlay).toHaveLength(1);
  await runtime.operations.put(intent);
  await runtime.operations.accept(operationId, { revision: 1, ack: {}, owners: publication(), overlay });
  const thread = { kind: "thread", id: params.thread_id };
  const taskName = taskTarget && { kind: "task", id: taskTarget.row.id };
  const refresh = taskName ? (taskFirst ? [taskName, thread] : [thread, taskName]) : [thread];
  const frame = { type: "writes.confirmed", operation_id: operationId, bridge_epoch: greeting.bridge_epoch,
    revision: 2, state: "succeeded", published: publication(), refresh };
  const record = await runtime.operations.confirm(operationId, { revision: frame.revision, confirmation: frame, refreshPending: true });
  const item = { type: "message", data: { sequence: 2, role: "user", body: "Canonical thread message",
    operation_id: operationId } };
  const page = { items: [oldItem, item], has_more: false, thread_total: 2, thread_last_sequence: 2,
    thread_id: params.thread_id, thread_generation_revision: params.thread_generation_revision };
  return { address, record, frame, page, params };
}

it.each([true, false])("refreshes every task and thread target of a mixed named obligation with taskFirst=%s", async (taskFirst) => {
  const writer = await independentRuntime(), target = await seedUncertainTask(writer, "mixed-task");
  const other = await seedUncertainTask(writer, "mixed-other-task");
  await confirmedOperation(writer, [other], { operationId: "mixed-other-operation" });
  const unrelated = await snapshot(writer, [other], "mixed-other-operation");
  const mixed = await confirmedThreadOperation(writer, { operationId: "mixed-operation", taskTarget: target, taskFirst });
  const authority = { active: () => true, call: vi.fn(async (method) => {
    if (method === "tasks.get") return answerFor(target);
    if (method === "thread.page") return mixed.page;
    throw new Error(`Unexpected mixed refresh RPC: ${method}`);
  }) };
  const result = await writer.refresh.refreshWriteOperation({ cache: writer.operations, ...mixed, authority });
  // The actual task read must first have admitted useful payload. The next
  // assertion witnesses the skipped supported named target, not a new API.
  expect((await writer.local.readCanonicalCached(target.address)).value.task.body).toBe("Admitted body");
  expect(authority.call).toHaveBeenCalledWith("thread.page", { entity_id: mixed.params.entity_id,
    conversation_id: mixed.params.conversation_id, thread_id: mixed.params.thread_id,
    thread_generation_revision: 3, newest: true, limit: 100 });
  expect(authority.call).toHaveBeenCalledTimes(2);
  const thread = await writer.local.readCanonicalCached(mixed.address);
  expect(thread.value.items).toEqual(mixed.page.items);
  expect(thread.value.thread_id).toBe(mixed.params.thread_id);
  expect(thread.value.thread_generation_revision).toBe(3);
  expect(thread.publication).toEqual(mixed.frame.published);
  expect(result.complete).toBe(false);
  const row = await writer.operations.get("mixed-operation");
  expect(row.refreshPending).toBe(true);
  expect(row.overlayActive).toBe(true);
  expect(row.overlay).toEqual(mixed.record.overlay);
  expect(row.refreshState.targets).toHaveLength(2);
  expect(row.refreshState.targets).toContainEqual(expect.objectContaining({ address: target.address, phase: "blocked", receipt: null }));
  expect(row.refreshState.targets).toContainEqual(expect.objectContaining({ address: mixed.address, phase: "receipt",
    receipt: expect.objectContaining({ revision: mixed.frame.revision, generation: row.refreshState.generation,
      targetKey: writer.keys.recordKey(mixed.address), required: mixed.frame.published }) }));
  // A retained row flag is insufficient: covering only the thread must not
  // make projection suppress this operation's still-required accepted layer.
  const paint = await writer.local.readCached(mixed.address);
  expect(paint.accepted).toBe(true);
  expect(paint.value.items).toEqual(mixed.page.items);
  expect(await snapshot(writer, [other], "mixed-other-operation")).toEqual(unrelated);
});

it("preserves a genuinely qualified thread-only refresh through service completion", async () => {
  const writer = await independentRuntime();
  const thread = await confirmedThreadOperation(writer, { operationId: "qualified-thread-operation" });
  const calls = [];
  const running = await startService(writer, () => { throw new Error("Unexpected task read"); }, calls,
    { read: async (method) => {
      expect(method).toBe("thread.page");
      return thread.page;
    } });
  expect(calls.filter(({ method }) => method === "thread.page")).toHaveLength(1);
  const canonical = await writer.local.readCanonicalCached(thread.address);
  expect(canonical.value.items).toEqual(thread.page.items);
  expect(canonical.publication).toEqual(thread.frame.published);
  expect(await writer.operations.get("qualified-thread-operation")).toMatchObject({ refreshPending: false, overlayActive: false });
  expect((await writer.local.readCached(thread.address)).accepted).not.toBe(true);
  expect(running.timers.size).toBe(0);
});

it.each(["copied", "serialized", "foreign-runtime"])("does not qualify an owned thread publication from a %s read entry", async (kind) => {
  const writer = await independentRuntime();
  const reader = kind === "foreign-runtime" ? await independentRuntime() : writer;
  const target = await seedTask(writer, "thread-brand-task");
  const mixed = await confirmedThreadOperation(writer, { operationId: "thread-brand-operation", taskTarget: target });
  const before = await writer.local.readCanonicalCached(mixed.address);
  expect(before.publication || []).toEqual([]);
  let original, forwarded;
  const cache = { get: writer.operations.get, openRefresh: writer.operations.openRefresh,
    observePublication: async (input) => {
      if (input.canonical[0].address.kind !== "thread") return writer.operations.observePublication(input);
      original = input.canonical[0];
      forwarded = kind === "copied" ? { ...original } : kind === "serialized" ? JSON.parse(JSON.stringify(original)) : original;
      return writer.operations.observePublication({ ...input, canonical: [forwarded] });
    } };
  const authority = { active: () => true, call: vi.fn(async (method) => method === "tasks.get" ? answerFor(target) : mixed.page) };
  const result = await reader.refresh.refreshWriteOperation({ cache, ...mixed, authority })
    .then((value) => ({ value }), (error) => ({ error }));
  expect(authority.call).toHaveBeenCalledTimes(2);
  expect(authority.call).toHaveBeenCalledWith("thread.page", expect.objectContaining({ thread_id: mixed.params.thread_id,
    thread_generation_revision: 3 }));
  expect(original).toBeDefined();
  expect(forwarded).toBeDefined();
  if (kind !== "foreign-runtime") expect(forwarded).not.toBe(original);
  const canonical = await writer.local.readCanonicalCached(mixed.address);
  // The actual read and private attempt may admit useful guarded thread paint.
  // A copied entry cannot qualify that paint as a full vector or owned receipt.
  // This first semantic oracle does not demand whole-record/event refusal.
  expect(canonical.publication || []).toEqual([]);
  expect(result.value?.complete).not.toBe(true);
  const operation = await writer.operations.get("thread-brand-operation");
  expect(operation).toMatchObject({ refreshPending: true, overlayActive: true });
  const slot = operation.refreshState.targets.find(({ address }) => writer.keys.recordKey(address) === writer.keys.recordKey(mixed.address));
  expect(slot.receipt).toBeNull();
  expect((await writer.local.readCached(mixed.address)).accepted).toBe(true);
});

it("does not convert a nonowned thread publication into an owned mixed receipt", async () => {
  const writer = await independentRuntime(), target = await seedUncertainTask(writer, "nonowned-thread-task");
  const mixed = await confirmedThreadOperation(writer, { operationId: "nonowned-thread-operation", taskTarget: target });
  const cache = { get: writer.operations.get, openRefresh: writer.operations.openRefresh,
    observePublication: async (input) => {
      if (input.canonical[0].address.kind !== "thread") return writer.operations.observePublication(input);
      // Deliberately use the preserved general publication API: useful full
      // thread paint is allowed, but no owned permit or reference is supplied.
      const { refresh: attempt, ...nonowned } = input;
      expect(attempt).toBeDefined();
      return writer.operations.observePublication(nonowned);
    } };
  const authority = { active: () => true, call: vi.fn(async (method) => method === "tasks.get" ? answerFor(target) : mixed.page) };
  const result = await writer.refresh.refreshWriteOperation({ cache, ...mixed, authority });
  expect(authority.call).toHaveBeenCalledTimes(2);
  const canonical = await writer.local.readCanonicalCached(mixed.address);
  expect(canonical.value.items).toEqual(mixed.page.items);
  expect(canonical.publication).toEqual(mixed.frame.published);
  const operation = await writer.operations.get("nonowned-thread-operation");
  expect(operation).toMatchObject({ refreshPending: true, overlayActive: true });
  const slot = operation.refreshState.targets.find(({ address }) => writer.keys.recordKey(address) === writer.keys.recordKey(mixed.address));
  expect(slot.receipt).toBeNull();
  expect(result.complete).toBe(false);
  expect((await writer.local.readCached(mixed.address)).accepted).toBe(true);
});

async function projectlessConfirmedOperation(runtime, target) {
  const { normalizeAckEffects } = await import("../src/core/writeOperationEffects.js");
  const operationId = "projectless-operation";
  const intent = { operationId, revision: 0, deviceId: scope.deviceId, journalIncarnation: greeting.journal_incarnation,
    request: { method: "tasks.comment", params: { task_id: target.row.id, body: "Accepted draft" } } };
  const ack = { result: { overlay: [{ kind: "task_comment", task_id: target.row.id,
    comment_id: `${target.row.id}-comment`, body: "Accepted draft" }] } };
  const normalized = await normalizeAckEffects(ack, intent);
  expect(normalized.overlay).toHaveLength(1);
  expect(normalized.overlay[0].address).toEqual(target.address);
  await runtime.operations.put(intent);
  await runtime.operations.accept(operationId, { ...normalized, revision: 1, ack, owners: [] });
  const frame = { type: "writes.confirmed", operation_id: operationId, bridge_epoch: greeting.bridge_epoch,
    revision: 2, state: "succeeded", published: [], refresh: [{ kind: "task", id: target.row.id }] };
  const record = await runtime.operations.confirm(operationId, { revision: frame.revision, confirmation: frame, refreshPending: true });
  expect(record.owner).toBeUndefined();
  expect(record.request.params.project_id).toBeUndefined();
  expect(record.owners).toEqual([]);
  expect(record.confirmation.published).toEqual([]);
  return { frame, record };
}

async function cachedTaskMatches(runtime, id) {
  return (await runtime.local.cachedAddresses({ deviceId: scope.deviceId }))
    .filter((address) => address.kind === runtime.tracker.TRACKER_TASK_KIND && address.sub === id);
}

it("refreshes a projectless operation through its unique genuine cached task address", async () => {
  const writer = await independentRuntime(), target = await seedUncertainTask(writer, "projectless-task");
  const confirmed = await projectlessConfirmedOperation(writer, target);
  expect(await cachedTaskMatches(writer, target.row.id)).toEqual([target.address]);
  expect(await writer.watch.readTaskWatchState(scope)).toMatchObject({ uncertain: true, eligible: false });
  const answer = answerFor(target);
  const authority = { active: () => true, call: vi.fn(async () => answer) };
  const result = await writer.refresh.refreshWriteOperation({ cache: writer.operations, ...confirmed, authority });
  // Actual cached address resolution must reach the supported literal reader;
  // this is the first expected RED on the 9ada validator, before payload checks.
  expect(authority.call).toHaveBeenCalledExactlyOnceWith("tasks.get", { task_id: target.row.id });
  const canonical = await writer.local.readCanonicalCached(target.address);
  expect(canonical.value.task.body).toBe(answer.task.body);
  expect(canonical.value.timeline).toEqual(answer.timeline);
  expect(canonical.publication || []).toEqual([]);
  expect(result.complete).toBe(false);
  const row = await writer.operations.get(confirmed.record.operationId);
  expect(row).toMatchObject({ refreshPending: true, overlayActive: true });
  expect(row.refreshState.targets).toContainEqual(expect.objectContaining({ address: target.address, phase: "blocked", receipt: null }));
  const paint = await writer.local.readCached(target.address);
  expect(paint.accepted).toBe(true);
  expect(paint.value.timeline.find(({ id }) => id === `${target.row.id}-comment`).body).toBe("Accepted draft");
});

it("refuses an ambiguous cached task scope for a projectless operation", async () => {
  const writer = await independentRuntime(), target = await seedTask(writer, "ambiguous-projectless-task");
  const confirmed = await projectlessConfirmedOperation(writer, target);
  const projectId = "other-payload-project", row = { ...target.row, project_id: projectId };
  const other = writer.tracker.taskAddress(scope.deviceId, projectId, row.id);
  const contentRead = await writer.content.captureTaskContentRead(other, { revalidate: "full" });
  await writer.tracker.writeTaskRecord(scope.deviceId, projectId, row.id, writer.tracker.taskRecord(row, []), { contentRead });
  expect((await writer.local.readCanonicalCached(other)).value.task.project_id).toBe(projectId);
  expect(await cachedTaskMatches(writer, row.id)).toHaveLength(2);
  const addresses = [target.address, other];
  const before = await Promise.all(addresses.map((address) => writer.local.readCanonicalCached(address)));
  const beforeOperation = await writer.operations.get(confirmed.record.operationId);
  const authority = { active: () => true, call: vi.fn(async () => answerFor(target)) };
  const result = await writer.refresh.refreshWriteOperation({ cache: writer.operations, ...confirmed, authority })
    .then((value) => ({ value }), (error) => ({ error }));
  expect(result.error?.message).toContain("cannot be mapped to a known record");
  expect(authority.call).not.toHaveBeenCalled();
  expect(await Promise.all(addresses.map((address) => writer.local.readCanonicalCached(address)))).toEqual(before);
  expect(await writer.operations.get(confirmed.record.operationId)).toEqual(beforeOperation);
});

it("refuses a caller supplied project that differs from the unique cached task scope", async () => {
  const writer = await independentRuntime(), target = await seedTask(writer, "mismatched-projectless-task");
  const confirmed = await projectlessConfirmedOperation(writer, target);
  expect(await cachedTaskMatches(writer, target.row.id)).toEqual([target.address]);
  const before = await writer.local.readCanonicalCached(target.address);
  const wrong = writer.tracker.taskAddress(scope.deviceId, "uncached-wrong-project", target.row.id);
  expect(await writer.local.readCanonicalCached(wrong)).toBeUndefined();
  const opening = await writer.operations.openRefresh({ operationId: confirmed.record.operationId,
    revision: confirmed.frame.revision, targets: [wrong], accept: () => true });
  expect(opening.state).toBe("unbound");
  expect(opening.attempt).toBeUndefined();
  expect(await writer.local.readCanonicalCached(target.address)).toEqual(before);
  expect(await writer.local.readCanonicalCached(wrong)).toBeUndefined();
  expect(await writer.operations.get(confirmed.record.operationId)).toEqual(confirmed.record);
});

// These inherited refresh contracts now exercise the real service and cache;
// a resolved partial result cannot retire either the obligation or its layer.
it.each([
  "keeps retrying when an intervening canonical write prevented layer retirement",
  "retains a required refresh when a newer task detail or canonical write rejects publication",
])("%s", async () => {
  const writer = await independentRuntime(), target = await seedTask(writer);
  await confirmedOperation(writer, [target]);
  const newer = { ...target.row, title: "Newer canonical title", body: "Newer canonical body",
    updated_at: "2026-11-01T00:00:00Z" };
  const contentRead = await writer.content.captureTaskContentRead(target.address, { revalidate: "full" });
  await writer.tracker.writeTaskRecord(scope.deviceId, scope.projectId, target.row.id,
    writer.tracker.taskRecord(newer, []), { contentRead });
  const parked = vi.spyOn(writer.operations, "watchRefresh");
  let observed, stop;
  const consumed = Promise.withResolvers();
  const running = await startService(writer, () => answerFor(target), [], {
    observation: { errors: [], refreshed: ({ result }) => { observed = result; },
      created: ({ service }) => { stop = service.subscribe((rows) => {
        if (observed) queueMicrotask(() => consumed.resolve(structuredClone(rows)));
      }); } },
  });
  const rows = await consumed.promise; stop();
  expect(rows).toMatchObject([{ refreshPending: true, overlayActive: true }]);
  expect(parked).toHaveBeenCalledExactlyOnceWith({ operationId: "payload-operation",
    generation: rows[0].refreshState.generation }, expect.any(Function));
  expect(observed).toMatchObject({ complete: false, refresh: { state: "blocked" },
    outcomes: [{ historyWritten: true, receiptPublished: false }] });
  await operationUnfinished(writer);
  const saved = await writer.local.readCanonicalCached(target.address);
  expect(saved.value.task).toMatchObject({ title: newer.title, body: newer.body });
  expect(saved.value.timeline).toEqual(answerFor(target).timeline);
  expect(saved.publication || []).toEqual([]);
  expect(running.timers.size).toBe(0);
  await running.service.reconcile(); await running.drain();
  expect(running.calls.filter(({ method }) => method === "tasks.get")).toHaveLength(1);
  await operationUnfinished(writer);
  expect((await writer.local.readCanonicalCached(target.address)).value.timeline).toEqual(answerFor(target).timeline);
});
