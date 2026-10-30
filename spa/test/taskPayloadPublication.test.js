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
  // The current protocol retains overlayActive even for an empty layer list;
  // empty effects do not finish the separately named refresh obligation.
  expect(record.overlayActive).toBe(true);
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

async function startService(runtime, answer, calls = []) {
  const pending = new Set(), timers = new Map();
  let timerId = 0;
  const session = { onWriteFrame: () => () => {}, callRaw: vi.fn(async (method, params, options) => {
    calls.push({ method, params });
    options?.assertCurrent?.();
    if (method.startsWith("writes.")) return { journal_incarnation: greeting.journal_incarnation, operations: [] };
    if (method === "tasks.get") return answer(params.task_id);
    return { ok: true, bridge_epoch: greeting.bridge_epoch, write: { phase: "ack", operation_id: options.write.operation_id,
      revision: 1, state: "accepted", accepted_epoch: greeting.bridge_epoch }, result: { saved: true } };
  }) };
  const service = runtime.operationService.createWriteOperationService({ ...identity, cache: runtime.operations,
    setTimeout: (run, wait) => { const id = ++timerId; timers.set(id, { run, wait }); return id; },
    clearTimeout: (id) => timers.delete(id), random: () => .5,
    onRefresh: (_addresses, frame, record, authority) => {
      const reading = runtime.refresh.refreshWriteOperation({ cache: runtime.operations, frame, record, authority });
      pending.add(reading);
      reading.then(() => pending.delete(reading), () => pending.delete(reading));
      return reading;
    },
  });
  const drain = async () => {
    while (pending.size) await Promise.allSettled([...pending]);
    // A real store turn also passes the service's promise-finally callbacks.
    await runtime.local.transactWriteOperations("readonly", () => null);
  };
  const running = { service, session, calls, timers, drain };
  services.push(running);
  await service.bind({ session, greeting });
  await drain();
  return running;
}

it("publishes admitted task payload under unknown watch authority without a receipt or completion", async () => {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const target = await seedTask(writer), { record, frame } = await confirmedOperation(writer, [target]);
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
  const writer = await independentRuntime(), target = await seedTask(writer);
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

it("does not wake a blocked obligation for unrelated or still unknown cache evidence", async () => {
  const writer = await independentRuntime(), target = await seedTask(writer);
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
  // Genuine positive D3-08 receipt integration is NOT RUN: its supported D1
  // as-of producer is not implemented. No private known-zero seed replaces it.
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
  expect(entry.contentRead.ticket.watchUncertain).toBe(true);
  await writer.operations.observePublication({ canonical: [entry] });
  const held = await writer.local.readCanonicalCached(target.address);
  expect(held.value.task.body).toBe(entry.value.task.body);
  expect(held.publication || []).toEqual([]);
  await operationUnfinished(writer, "payload-operation", record.overlayActive);
  expect(await writer.watch.readTaskWatchState(scope)).toMatchObject({ uncertain: true, eligible: false });
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
