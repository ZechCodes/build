import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
let cache, content, order;
const scope = { deviceId: "content-model-900", projectId: "proj-1" };
const address = { deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-task", sub: "task-1" };
const task = (extra = {}) => ({ id: "task-1", project_id: scope.projectId, number: 1, title: "Current", body: "Body",
  state: "open", status: "ready", attachments: [], updated_at: "2026-10-29T13:00:00.000900Z", ...extra });
const read = async () => (await cache.readCached(address))?.value?.task;
async function publish(candidate, headerBarrier, headerGuards) {
  let result;
  await content.mergeTaskContentRecordsTogether(scope, [address], ([held], context) => {
    result = content.reconcileTaskContent(held?.value?.task, candidate,
      { context, address, heldWritten: cache.recordWriteOf(held), headerBarrier, headerGuards });
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

it("refuses shared content counter overflow without replacing the stored safe high-water value", async () => {
  await cache.writeCached(order.TASK_READ_COUNT_ADDRESS, Number.MAX_SAFE_INTEGER);
  await expect(content.captureTaskContentRead(scope)).rejects.toThrow(/counter|allocation/i);
  expect((await cache.readCached(order.TASK_READ_COUNT_ADDRESS)).value).toBe(Number.MAX_SAFE_INTEGER);
});

it("keeps retirement recovery identity stable across identical newer header observations", async () => {
  await publish(await observed(task()));
  await content.mergeTaskContentRecordsTogether(scope, [address], ([held]) => ({
    values: [{ task: content.retireTaskContent(held.value.task, "body") }], retire: true,
  }));
  const key = content.taskFullRefreshKey(await read()); expect(key).not.toBeNull();
  for (let index = 0; index < 3; index += 1) {
    const candidate = content.observeTaskContent(task(), { contentRead: await content.captureTaskContentRead(scope,
      { revalidate: "summary" }), coverage: "summary" });
    await publish(candidate); expect(content.taskFullRefreshKey(await read())).toBe(key);
  }
});

it("leaves a summary header body-free until an explicitly wanted missing detail asks for recovery", async () => {
  const fragment = content.taskContentFragment(scope), captured = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const header = content.reconcileTaskContent(undefined, content.observeTaskContent(task(), { contentRead: captured, coverage: "summary" }),
    { context: fragment.context(await cache.readCachedMany(fragment.addresses)) }).task;
  expect(content.taskNeedsFullRefresh(header)).toBe(false);
  expect(content.taskNeedsFullRefresh(undefined, header)).toBe(true);
  expect(content.taskFullRefreshKey(undefined, header)).not.toBeNull();
});

it("bars an intermediate full observation behind the same record's newer accepted header", async () => {
  await publish(await observed(task({ title: "Header two", body: "Body two", updated_at: "2026-10-29T13:00:00.000100Z" })));
  const middle = content.observeTaskContent(task({ title: "Header four", body: "Body four", updated_at: "2026-10-29T13:00:00.000500Z" }),
    { contentRead: await content.captureTaskContentRead(scope), coverage: "full" });
  const header = content.observeTaskContent(task({ title: "Header five" }),
    { contentRead: await content.captureTaskContentRead(scope, { revalidate: "summary" }), coverage: "summary" });
  await publish(header);
  const result = await publish(middle);
  expect(result.acceptedHeader).toBe(false);
  expect(result.acceptedBody).toBe(false);
  expect(result.task).toMatchObject({ title: "Header five", body: "Body two" });
  expect(result.needsFullRefresh).toBe(true);
  const fresh = await publish(await observed(task({ title: "Header five", body: "Body six" })));
  expect(fresh.acceptedBody).toBe(true);
  expect(fresh.needsFullRefresh).toBe(false);
});

it("associates independently published identical headers without a permanent detail refresh loop", async () => {
  const headerAddress = { ...address, kind: "tracker-summary" };
  const publishHeader = async (raw) => {
    const captured = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    await content.mergeTaskContentRecordsTogether(scope, [headerAddress], ([held], context) => ({
      values: [{ task: content.reconcileTaskContent(held?.value?.task,
        content.observeTaskContent(raw, { contentRead: captured, coverage: "summary" }), { context }).task }],
    }));
    return (await cache.readCached(headerAddress)).value.task;
  };
  const first = await publishHeader(task());
  const full = await publish(await observed(task()), first);
  expect(content.taskNeedsFullRefresh(full.task, first)).toBe(false);
  const same = await publishHeader(task());
  expect(content.taskNeedsFullRefresh(full.task, same)).toBe(false);
  const changed = await publishHeader(task({ title: "Changed" }));
  expect(content.taskNeedsFullRefresh(full.task, changed)).toBe(true);
  const key = content.taskFullRefreshKey(full.task, changed);
  expect(content.taskFullRefreshKey(full.task, await publishHeader(task({ title: "Changed" })))).toBe(key);
});

it("covers an actual full TaskView after its compact summary header without a shape-induced refresh loop", async () => {
  const fixture = (await import("../../fixtures/api/v1/tasks.read_through.json")).default.result.task;
  const full = { ...fixture, id: "task-1", project_id: scope.projectId };
  // TaskSummaryView adds compact counts/flags absent from the full TaskView.
  const compact = { ...full, attachment_count: 0, summary_truncated: false, done_at: null,
    dependencies: { blocked: false, waiting_on_count: 0, blocks_count: 0 } };
  for (const field of ["body", "attachments", "identities"]) delete compact[field];
  compact.links = { workspace_ids: full.links.workspace_ids, parent_task_id: full.links.parent_task_id };
  const captured = await content.captureTaskContentRead(scope, { revalidate: "summary" }), fragment = content.taskContentFragment(scope);
  const header = content.reconcileTaskContent(undefined, content.observeTaskContent(compact, { contentRead: captured, coverage: "summary" }),
    { context: fragment.context(await cache.readCachedMany(fragment.addresses)) }).task;
  const result = await publish(await observed(full), header);
  expect(result.acceptedBody).toBe(true);
  expect(content.taskNeedsFullRefresh(result.task, header)).toBe(false);
  expect(content.taskFullRefreshKey(result.task, header)).toBeNull();
});

it("refuses an absent summary row after an unrelated blind rewrite changes its captured domain", async () => {
  const summaries = await import("../src/core/taskSummaryCache.js"), tracker = await import("../src/core/trackerCache.js");
  const ticket = await summaries.beginSummaryQuery(scope, {}, await order.nextTaskRead());
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [task({ id: "other-task" })] });
  expect(await summaries.landSummaryPage(ticket, { summary: true, basis: "snapshot", tasks: [task()] },
    { read: ticket.generation })).toBe(false);
  expect(await cache.readCached(summaries.summaryAddress(scope, "task-1"))).toBeUndefined();
  expect((await summaries.readSummaryQuery(scope, {})).refreshing).toBe(true);
});

it("never certifies a fallback-only allocation when shared storage later recovers", async () => {
  await publish(await observed(task()));
  const unavailable = vi.spyOn(order, "nextTaskReadTicket").mockResolvedValueOnce(null);
  const pending = await observed(task({ title: "Fallback", body: "Fallback" }));
  unavailable.mockRestore();
  await content.captureTaskContentRead(scope);
  const result = await publish(pending);
  expect(result).toMatchObject({ acceptedHeader: false, acceptedBody: false });
  expect(result.task).toMatchObject({ title: "Current", body: "Body" });
});

it("refuses a corrupt fractional shared count without writing a replacement authority", async () => {
  await cache.writeCached(order.TASK_READ_COUNT_ADDRESS, 1.5);
  await expect(content.captureTaskContentRead(scope)).rejects.toThrow(/counter|allocation/i);
  expect((await cache.readCached(order.TASK_READ_COUNT_ADDRESS)).value).toBe(1.5);
  expect(await cache.readCached(order.TASK_CONTENT_GENERATION_ADDRESS)).toBeUndefined();
});

it("does not attribute a newly accepted body to timestamp-protected retired header bytes", async () => {
  await publish(await observed(task({ updated_at: "2026-10-29T10:00:00Z" })));
  await publish(content.observeTaskContent(task({ title: "Future header", updated_at: "2026-10-29T20:00:00Z" }),
    { contentRead: await content.captureTaskContentRead(scope, { revalidate: "summary" }), coverage: "summary" }));
  await content.mergeTaskContentRecordsTogether(scope, [address], ([held]) => ({
    values: [{ task: content.retireTaskContent(held.value.task, "header") }], retire: true,
  }));
  const result = await publish(await observed(task({ title: "Intermediate", body: "Intermediate", updated_at: "2026-10-29T15:00:00Z" })));
  expect(result).toMatchObject({ acceptedHeader: false, acceptedBody: false, needsFullRefresh: true });
  expect(result.task).toMatchObject({ title: "Future header", body: "Body" });
});

it("gives each explicit header retirement its own stable bounded recovery identity", async () => {
  const held = (await publish(await observed(task()))).task;
  const first = content.retireTaskContent(held, "header"), second = content.retireTaskContent(first, "header");
  expect(content.taskFullRefreshKey(first)).not.toBeNull();
  expect(content.taskFullRefreshKey(second)).not.toBe(content.taskFullRefreshKey(first));
  const barrierOne = content.retireTaskContent(held, "header"), barrierTwo = content.retireTaskContent(barrierOne, "header");
  const key = content.taskFullRefreshKey(held, barrierOne);
  expect(content.taskFullRefreshKey(held, { ...barrierOne, unread_count: 99 })).toBe(key);
  expect(content.taskFullRefreshKey(held, barrierTwo)).not.toBe(key);
  expect(content.taskFullRefreshKey(second, barrierOne)).not.toBe(content.taskFullRefreshKey(first, barrierOne));
});

it("publishes independent PR and read facts beside the selected ordinary detail and list components", async () => {
  const tracker = await import("../src/core/trackerCache.js"), listAddress = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const raw = (version, mark) => task({ read_through: mark, unread_count: 3 - version, watched: true,
    review_summary: { task_id: "task-1", version, status: "open", mode: "pull_request" } });
  for (const version of [1, 2]) {
    const next = raw(version, `tc-${version}0`);
    await tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-1", { task: next, timeline: [] },
      { contentRead: await content.captureTaskContentRead(address, { revalidate: "full" }) });
    const contentRead = await content.captureTaskContentRead(listAddress, { revalidate: "full" });
    await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([
      content.observeTaskContent(next, { contentRead, coverage: "full" }),
    ], [], contentRead.readOrder));
  }
  const rows = [(await tracker.readTaskRecord(scope.deviceId, scope.projectId, "task-1")).task,
    (await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0]];
  for (const row of rows) expect(row).toMatchObject({ title: "Current", body: "Body", read_through: "tc-20",
    unread_count: 1, review_summary: { version: 2 } });
});

it("recovers a retired normalized header through the existing addressed full-detail owner", async () => {
  const summaries = await import("../src/core/taskSummaryCache.js"), tracker = await import("../src/core/trackerCache.js");
  const seed = task();
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, seed.id, { task: seed, timeline: [] },
    { contentRead: await content.captureTaskContentRead(address, { revalidate: "full" }) });
  const ticket = await summaries.beginSummaryQuery(scope, {}, await order.nextTaskRead());
  await summaries.landSummaryPage(ticket, { summary: true, basis: "before", tasks: [seed] }, { read: ticket.generation });
  const retireHeader = async () => {
    const held = (await cache.readCached(summaries.summaryAddress(scope, seed.id))).value.task;
    await summaries.writeSummaryTask(scope, content.retireTaskContent({ ...held, title: "Optimistic title" }, "header"), await order.nextTaskRead());
  };
  await retireHeader();
  const before = await tracker.readTaskRecord(scope.deviceId, scope.projectId, seed.id);
  expect(content.taskFullRefreshKey(before.task)).not.toBeNull();
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, seed.id, { task: seed, timeline: [] },
    { contentRead: await content.captureTaskContentRead(address, { revalidate: "full" }) });
  const recovered = await tracker.readTaskRecord(scope.deviceId, scope.projectId, seed.id);
  expect(content.taskNeedsFullRefresh(recovered.task)).toBe(false);
  expect(content.taskFullRefreshKey(recovered.task)).toBeNull();
  // Covering one retirement never covers a subsequent optimistic rewrite.
  await retireHeader();
  expect(content.taskFullRefreshKey((await tracker.readTaskRecord(scope.deviceId, scope.projectId, seed.id)).task)).not.toBeNull();
});

it("keeps a retired external future-header timestamp protective during addressed full recovery", async () => {
  const summaries = await import("../src/core/taskSummaryCache.js");
  await publish(await observed(task({ updated_at: "2026-10-29T10:00:00Z" })));
  const future = task({ title: "Future header", updated_at: "2026-10-29T20:00:00Z" });
  const ticket = await summaries.beginSummaryQuery(scope, {}, await order.nextTaskRead());
  await summaries.landSummaryPage(ticket, { summary: true, basis: "future", tasks: [future] }, { read: ticket.generation });
  const summaryAddress = summaries.summaryAddress(scope, future.id);
  const held = (await cache.readCached(summaryAddress)).value.task;
  await summaries.writeSummaryTask(scope, content.retireTaskContent(held, "header"), await order.nextTaskRead());
  const barrier = (await cache.readCached(summaryAddress)).value.task;
  const result = await publish(await observed(task({ title: "Intermediate", body: "Intermediate", updated_at: "2026-10-29T15:00:00Z" })), barrier);
  expect(result).toMatchObject({ acceptedHeader: false, acceptedBody: false, needsFullRefresh: true });
  expect(result.task).toMatchObject({ title: "Current", body: "Body" });
});

it("keeps accepted same-byte component clocks when an older cached copy is republished", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const first = (await publish(await observed(task()))).task;
  const pending = content.observeTaskContent(task({ title: "Earlier action", body: "Earlier action" }),
    { contentRead: await content.captureTaskContentRead(scope), coverage: "full" });
  const current = (await publish(await observed(task()))).task;
  const fragment = content.taskContentFragment(scope), before = fragment.context(await cache.readCachedMany(fragment.addresses));
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-1", { task: first, timeline: [] });
  const copied = await read();
  expect(copied.__task_content_cache.header).toEqual(current.__task_content_cache.header);
  expect(copied.__task_content_cache.body).toEqual(current.__task_content_cache.body);
  expect(fragment.context(await cache.readCachedMany(fragment.addresses))).toEqual(before);
  const result = await publish(pending);
  expect(result).toMatchObject({ acceptedHeader: false, acceptedBody: false });
  expect(result.task).toMatchObject({ title: "Current", body: "Body" });
});

it("does not promote a retired unknown body by copying an older proven identical body", async () => {
  const tracker = await import("../src/core/trackerCache.js"), first = (await publish(await observed(task()))).task;
  await content.mergeTaskContentRecordsTogether(scope, [address], ([held]) => ({
    values: [{ task: content.retireTaskContent(held.value.task, "body") }], retire: true,
  }));
  const unknown = await read(), key = content.taskFullRefreshKey(unknown);
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-1", { task: first, timeline: [] });
  const copied = await read();
  expect(copied.__task_content_cache.body).toBeUndefined();
  expect(content.taskFullRefreshKey(copied)).toBe(key);
});

it("selects genuine same-byte copy proof per component across cache replicas", async () => {
  const tracker = await import("../src/core/trackerCache.js"), listAddress = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const initial = (await publish(await observed(task()))).task;
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [initial] });
  const newerBody = (await publish(await observed(task()))).task;
  const headerRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [
    content.observeTaskContent(task(), { contentRead: headerRead, coverage: "summary" }),
  ] });
  const before = (await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0];
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [newerBody] });
  const copied = (await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0];
  expect(copied.__task_content_cache.header).toEqual(before.__task_content_cache.header);
  expect(copied.__task_content_cache.body).toEqual(newerBody.__task_content_cache.body);
  expect(copied.__task_content_cache.bodyHeaderIdentity).toBe(newerBody.__task_content_cache.bodyHeaderIdentity);
});

it("distinguishes external barrier lifetimes for missing and retained detail recovery", async () => {
  const summaries = await import("../src/core/taskSummaryCache.js");
  const land = async () => {
    const ticket = await summaries.beginSummaryQuery(scope, {}, await order.nextTaskRead());
    await summaries.landSummaryPage(ticket, { summary: true, basis: "B", tasks: [task()] }, { read: ticket.generation });
    return (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.task;
  };
  const first = await land(), missingKey = content.taskFullRefreshKey(undefined, first);
  const retained = content.retireTaskContent(task(), "body"), retainedKey = content.taskFullRefreshKey(retained, first);
  expect(content.taskFullRefreshKey(undefined, await land())).toBe(missingKey);
  await cache.evictEntity(scope.deviceId, scope.projectId);
  const replacement = await land();
  expect(content.taskFullRefreshKey(undefined, replacement)).not.toBe(missingKey);
  expect(content.taskFullRefreshKey(retained, replacement)).not.toBe(retainedKey);
  expect(content.taskFullRefreshKey(undefined, await land())).toBe(content.taskFullRefreshKey(undefined, replacement));
});

it("paints an explicitly retired local full creation as an unknown normalized row", async () => {
  const summaries = await import("../src/core/taskSummaryCache.js"), local = content.retireTaskContent(task(), "full");
  const before = await content.captureTaskContentRead(scope);
  await summaries.writeSummaryTask(scope, local, await order.nextTaskRead(), { fullTask: true });
  const row = (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value;
  expect(row.task).toMatchObject({ id: "task-1", title: "Current" });
  expect(row.task.body).toBeUndefined(); expect(row.task.__task_content_cache.header).toBeUndefined();
  expect(row.task.__task_content_cache.retired.header).toBe(local.__task_content_cache.retired.header);
  const after = await content.captureTaskContentRead(scope);
  expect(after.ticket.retirementEpoch).not.toBe(before.ticket.retirementEpoch);
  expect(row.basis).toBeUndefined();
});

it("never turns an unticketed or retired-domain raw reply into a blind ordinary creation", async () => {
  const actions = await import("../src/core/taskWaitingOnCache.js"), summaries = await import("../src/core/taskSummaryCache.js"),
    tracker = await import("../src/core/trackerCache.js");
  const ticket = await summaries.beginSummaryQuery(scope, {}, await order.nextTaskRead());
  await summaries.landSummaryPage(ticket, { summary: true, basis: "held", tasks: [task()] }, { read: ticket.generation });
  const pending = await content.captureTaskContentRead(scope);
  // An actual unrelated local rewrite retires the dispatch domain.
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [task({ id: "other-task" })] });
  for (const options of [{}, { contentRead: pending, issuedRead: pending.readOrder }]) {
    await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: task({ title: "Stale raw" }) }, options);
    expect(await tracker.readTaskRecord(scope.deviceId, scope.projectId, "task-1")).toBeNull();
    expect((await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks.map((row) => row.id)).toEqual(["other-task"]);
    expect((await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.task.title).toBe("Current");
  }
});

it("admits a genuine absent action header and its immediate next assignment without body proof", async () => {
  const actions = await import("../src/core/taskWaitingOnCache.js"), summaries = await import("../src/core/taskSummaryCache.js"),
    tracker = await import("../src/core/trackerCache.js");
  const create = await content.captureTaskContentRead(scope);
  await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: task({ assignee: { kind: "user" } }) },
    { contentRead: create, issuedRead: create.readOrder });
  let row = (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.task;
  expect(row.__task_content_cache.header?.read_order).toBe(create.readOrder);
  expect(row.__task_content_cache.body).toBeUndefined(); expect(row.body).toBeUndefined();
  expect(await tracker.readTaskRecord(scope.deviceId, scope.projectId, "task-1")).toBeNull();
  const unassign = await content.captureTaskContentRead(scope);
  await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: task({ assignee: null }) },
    { contentRead: unassign, issuedRead: unassign.readOrder });
  row = (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.task;
  expect(row.assignee).toBeNull(); expect(row.__task_content_cache.header.read_order).toBe(unassign.readOrder);
  expect((await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0].assignee).toBeNull();
});

it("keeps existing unknown headers fenced while admitting only truly absent action headers", async () => {
  await cache.writeCached(address, { task: task(), timeline: [] });
  const result = await publish(content.observeTaskContent(task({ title: "Unproven replacement", body: "Unproven replacement" }),
    { contentRead: await content.captureTaskContentRead(scope), coverage: "full" }));
  expect(result).toMatchObject({ acceptedHeader: false, acceptedBody: false });
  expect(result.task).toMatchObject({ title: "Current", body: "Body" });
  expect(result.task.__task_content_cache.header).toBeUndefined();
});

it("fences a genuine absent action header behind a stronger external header without borrowing its proof", async () => {
  const pending = content.observeTaskContent(task({ title: "Earlier action" }),
    { contentRead: await content.captureTaskContentRead(scope), coverage: "full" });
  const headerRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const fragment = content.taskContentFragment(scope), context = fragment.context(await cache.readCachedMany(fragment.addresses));
  const barrier = content.reconcileTaskContent(undefined, content.observeTaskContent(task({ title: "Current barrier" }),
    { contentRead: headerRead, coverage: "summary" }), { context }).task;
  const result = await publish(pending, barrier);
  expect(result).toMatchObject({ acceptedHeader: false, acceptedBody: false });
  expect(result.task).toMatchObject({ title: "Current barrier" });
  expect(result.task.body).toBeUndefined(); expect(result.task.__task_content_cache.header).toBeUndefined();
});

it("does not promote a list-only unknown header by treating its missing normalized replica as absent", async () => {
  const tracker = await import("../src/core/trackerCache.js"), actions = await import("../src/core/taskWaitingOnCache.js"),
    summaries = await import("../src/core/taskSummaryCache.js");
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [task({ title: "Unknown held" })] });
  const contentRead = await content.captureTaskContentRead(scope);
  await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: task({ title: "Raw action" }) },
    { contentRead, issuedRead: contentRead.readOrder });
  const normalized = (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.task;
  expect(normalized.title).toBe("Unknown held");
  expect(normalized.__task_content_cache.header).toBeUndefined();
  expect((await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0].title).toBe("Unknown held");
});

it("retains a list-only newer header observation over an older action creating normalized state", async () => {
  const tracker = await import("../src/core/trackerCache.js"), actions = await import("../src/core/taskWaitingOnCache.js"),
    summaries = await import("../src/core/taskSummaryCache.js");
  const old = await content.captureTaskContentRead(scope);
  const listRead = await content.captureTaskContentRead(tracker.tasksAddress(scope.deviceId, scope.projectId), { revalidate: "full" });
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [content.observeTaskContent(task({ title: "Known newer" }),
    { contentRead: listRead, coverage: "full" })] });
  await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: task({ title: "Older raw" }) },
    { contentRead: old, issuedRead: old.readOrder });
  const normalized = (await cache.readCached(summaries.summaryAddress(scope, "task-1"))).value.task;
  expect(normalized.title).toBe("Known newer");
  expect(normalized.__task_content_cache.header.read_order).toBe(listRead.readOrder);
  expect((await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0].title).toBe("Known newer");
});

it("checks crossed replica timestamp and observation guards without synthesizing a header proof", async () => {
  const tracker = await import("../src/core/trackerCache.js"), firstAddress = tracker.tasksQueryAddress(scope.deviceId, scope.projectId, { state: "open" }),
    secondAddress = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const first = await content.captureTaskContentRead(firstAddress, { revalidate: "full" });
  await tracker.writeTasksQueryRecord(scope.deviceId, scope.projectId, { state: "open" }, { tasks: [
    content.observeTaskContent(task({ title: "Newer timestamp", updated_at: "2026-10-29T20:00:00Z" }), { contentRead: first, coverage: "full" }),
  ] });
  const second = await content.captureTaskContentRead(secondAddress, { revalidate: "full" });
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [
    content.observeTaskContent(task({ title: "Newer observation", updated_at: "2026-10-29T10:00:00Z" }), { contentRead: second, coverage: "full" }),
  ] });
  const guards = [(await tracker.readTasksQueryRecord(scope.deviceId, scope.projectId, { state: "open" })).tasks[0],
    (await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0]];
  const incoming = content.observeTaskContent(task({ title: "Intermediate timestamp", updated_at: "2026-10-29T15:00:00Z" }),
    { contentRead: await content.captureTaskContentRead(scope), coverage: "full" });
  const result = await publish(incoming, guards[1], guards);
  expect(result).toMatchObject({ acceptedHeader: false, acceptedBody: false });
  expect(result.task).toMatchObject({ title: "Newer observation", updated_at: "2026-10-29T10:00:00Z" });
  expect(result.task.__task_content_cache.header).toBeUndefined();
});

async function seededFullObservation(extra = {}) {
  const tracker = await import("../src/core/trackerCache.js"), summaries = await import("../src/core/taskSummaryCache.js"),
    { summaryPatch } = await import("../src/core/taskSummaryProjection.js");
  const initial = task({ watched: false, ...extra });
  const ticket = await summaries.beginSummaryQuery(scope, {}, await order.nextTaskRead());
  await summaries.landSummaryPage(ticket, { summary: true, basis: "B1", tasks: [summaryPatch(undefined, initial, true)] }, { read: ticket.generation });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id, { task: initial, timeline: ["Held history"] },
    { contentRead: await content.captureTaskContentRead(address, { revalidate: "full" }) });
  return { tracker, summaries, initial, summaryAddress: summaries.summaryAddress(scope, initial.id),
    invalidation: { deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-summary-invalidation", sub: "" } };
}

it("normalizes one admitted full read jointly with detail and covers its actual compact header without a recovery loop", async () => {
  const { tracker, summaries, initial, summaryAddress } = await seededFullObservation();
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  const incoming = { ...initial, title: "Canonical full read", body: "Canonical body", attachments: [{ path: "new-file" }] };
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id, { task: incoming, timeline: ["New history"] }, { contentRead })).toBe(true);
  const normalized = (await cache.readCached(summaryAddress)).value, detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, initial.id);
  expect(normalized.task).toMatchObject({ title: "Canonical full read", attachment_count: 1 });
  expect(normalized.task.body).toBeUndefined(); expect(normalized.task.__task_content_cache.body).toBeUndefined();
  expect(detail.task).toMatchObject({ title: "Canonical full read", body: "Canonical body" });
  expect(content.taskNeedsFullRefresh(detail.task, normalized.task)).toBe(false);
  expect(content.taskFullRefreshKey(detail.task, normalized.task)).toBeNull();
  expect(normalized.basis).toBeUndefined(); expect(normalized.basis_uncertain).toBe(true);
  expect(await summaries.readSummaryQuery(scope, {})).toMatchObject({ complete: true, stale: true, ids: [initial.id] });
});

it("rolls back full detail and normalized observation together on normalized quota refusal", async () => {
  const { tracker, initial, summaryAddress, invalidation } = await seededFullObservation();
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  const addresses = [address, summaryAddress, invalidation], before = await cache.readCachedMany(addresses);
  const { IDBObjectStore } = await import("fake-indexeddb"), put = IDBObjectStore.prototype.put;
  const spy = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function(record, key) {
    if (String(key).includes("|tracker-summary|") && record?.value?.task?.title === "Quota full read") throw new DOMException("Quota", "QuotaExceededError");
    return put.call(this, record, key);
  });
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id,
    { task: { ...initial, title: "Quota full read" }, timeline: ["Must roll back"] }, { contentRead })).toBe(false);
  spy.mockRestore(); expect(await cache.readCachedMany(addresses)).toEqual(before);
});

it("refuses every full-read output when the session changes during its joint cache reads", async () => {
  const { tracker, initial, summaryAddress, invalidation } = await seededFullObservation();
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  const addresses = [address, summaryAddress, invalidation], before = await cache.readCachedMany(addresses);
  let accepted = true;
  const { IDBObjectStore } = await import("fake-indexeddb"), get = IDBObjectStore.prototype.get;
  const spy = vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function(key) {
    if (String(key).includes("|tracker-summary|")) accepted = false;
    return get.call(this, key);
  });
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id,
    { task: { ...initial, title: "Wrong session" }, timeline: ["Wrong session"] }, { contentRead, accept: () => accepted })).toBe(false);
  spy.mockRestore(); expect(await cache.readCachedMany(addresses)).toEqual(before);
});

it("retains retired ordinary content while a stale full read independently advances read and review facts", async () => {
  const { tracker, initial, summaryAddress } = await seededFullObservation();
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id, { task: { ...initial, body: "Optimistic body" }, timeline: ["Held history"] });
  const before = (await cache.readCached(address)).value;
  const review_summary = { version: 2, state: "approved" };
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id,
    { task: { ...initial, title: "Stale full read", read_through: "tc-99", review_summary }, timeline: ["Stale history"] }, { contentRead })).toBe(true);
  const detail = (await cache.readCached(address)).value, normalized = (await cache.readCached(summaryAddress)).value;
  expect(detail.task).toMatchObject({ title: initial.title, body: "Optimistic body", read_through: "tc-99", review_summary });
  expect(detail.timeline).toEqual(["Held history"]);
  expect(detail.task.__task_content_cache.header).toEqual(before.task.__task_content_cache.header);
  expect(detail.task.__task_content_cache.body).toBeUndefined();
  expect(normalized.task).toMatchObject({ title: initial.title, read_through: "tc-99", review_summary });
  expect(normalized.task.__task_content_cache.header.read_order).toBeLessThan(contentRead.readOrder);
  expect(normalized.task.__task_content_cache.body).toBeUndefined();
  expect(normalized.basis_uncertain).toBe(true);
});

it("keeps a known summary basis only when the full read preserves every compact semantic field", async () => {
  const { tracker, summaries, initial, summaryAddress } = await seededFullObservation({
    read_through: null, dismissed_through: null, done_at: null, review_summary: null, deferred_assignment: null,
  });
  const { summaryPatch } = await import("../src/core/taskSummaryProjection.js");
  const page = await summaries.beginSummaryQuery(scope, {}, await order.nextTaskRead());
  await summaries.landSummaryPage(page, { summary: true, basis: "B2", tasks: [summaryPatch(undefined, initial, true)] }, { read: page.generation });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id, { task: initial, timeline: ["Refreshed history"] },
    { contentRead: await content.captureTaskContentRead(address, { revalidate: "full" }) });
  expect((await cache.readCached(summaryAddress)).value).toMatchObject({ basis: "B2", basis_uncertain: false });
  expect(await summaries.readSummaryQuery(scope, {})).toMatchObject({ stale: false, complete: true });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id, { task: { ...initial, read_through: "tc-02" }, timeline: [] },
    { contentRead: await content.captureTaskContentRead(address, { revalidate: "full" }) });
  expect((await cache.readCached(summaryAddress)).value).toMatchObject({ basis_uncertain: true });
  expect((await cache.readCached(summaryAddress)).value.basis).toBeUndefined();
  expect(await summaries.readSummaryQuery(scope, {})).toMatchObject({ stale: true, complete: true });
});

it("does not certify omitted watched counter families from a full read as complete unread authority", async () => {
  const { tracker, initial, summaryAddress } = await seededFullObservation({ watched: true, watch_started_after: "te-01",
    read_through: "tc-01", unread_count: 3, unread_mentions: 2, unread_agent_comments: 1 });
  const incoming = { ...initial, read_through: "tc-02", unread_count: 1 };
  delete incoming.unread_mentions; delete incoming.unread_agent_comments;
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id, { task: incoming, timeline: [] },
    { contentRead: await content.captureTaskContentRead(address, { revalidate: "full" }) });
  const normalized = (await cache.readCached(summaryAddress)).value;
  expect(normalized.task.read_through).toBe("tc-02");
  expect(normalized.needs_unread_refresh).toBe(true);
});

it("publishes eligible compact read and review facts when a newer summary bars a cold full detail body", async () => {
  const tracker = await import("../src/core/trackerCache.js"), summaries = await import("../src/core/taskSummaryCache.js");
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  const { summaryPatch } = await import("../src/core/taskSummaryProjection.js");
  const newer = task({ title: "Newer compact header", watched: false, read_through: "tc-10",
    review_summary: { task_id: "task-1", version: 1, status: "open", mode: "pull_request" } });
  const ticket = await summaries.beginSummaryQuery(scope, {}, await order.nextTaskRead());
  await summaries.landSummaryPage(ticket, { summary: true, basis: "newer", tasks: [summaryPatch(undefined, newer, true)] }, { read: ticket.generation });
  const review_summary = { ...newer.review_summary, version: 2 };
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, newer.id,
    { task: task({ title: "Old full header", watched: false, read_through: "tc-20", review_summary }), timeline: ["Old history"] }, { contentRead })).toBe(true);
  expect(await cache.readCached(address)).toBeUndefined();
  const normalized = (await cache.readCached(summaries.summaryAddress(scope, newer.id))).value;
  expect(normalized.task).toMatchObject({ title: "Newer compact header", read_through: "tc-20", review_summary });
  expect(normalized.task.__task_content_cache.header.read_order).toBe(ticket.contentRead.readOrder);
  expect(normalized.task.__task_content_cache.body).toBeUndefined();
  expect(normalized.basis_uncertain).toBe(true);
});

it("certifies only raw full TaskView compact fields and derives its complete waiting count without retaining summary-only header bytes", async () => {
  const { tracker, summaries, initial, summaryAddress } = await seededFullObservation();
  const { summaryPatch } = await import("../src/core/taskSummaryProjection.js");
  const ticket = await summaries.beginSummaryQuery(scope, {}, await order.nextTaskRead());
  await summaries.landSummaryPage(ticket, { summary: true, basis: "compact", tasks: [{ ...summaryPatch(undefined, initial, true),
    summary_truncated: true, dependencies: { blocked: true, waiting_on_count: 9, blocks_count: 8 } }] }, { read: ticket.generation });
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  const raw = { ...initial, dependencies: { blocked: true, waiting_on: [{ task_id: "dep-1" }], blocks_count: 3,
    blocks: [{ task_id: "child-1" }], blocks_next_cursor: "more-blocks" } };
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, initial.id, { task: raw, timeline: ["Canonical history"] }, { contentRead });
  const normalized = (await cache.readCached(summaryAddress)).value;
  expect(normalized.task.dependencies).toEqual({ blocked: true, waiting_on_count: 1, blocks_count: 3 });
  expect(normalized.task).not.toHaveProperty("summary_truncated");
  expect(normalized.task.__task_content_cache.header.read_order).toBe(contentRead.readOrder);
  expect(normalized.task).not.toHaveProperty("body");
  const detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, initial.id);
  expect(content.taskFullRefreshKey(detail.task, normalized.task)).toBeNull();
});
