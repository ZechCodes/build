import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

const scope = { deviceId: "paged-watch-device", projectId: "paged-watch-project" };
const params = { project_id: scope.projectId };
const row = (number = 20, over = {}) => task({ id: `task-${number}`, project_id: scope.projectId,
  number, title: "Held page title", attachments: [], watched: true,
  read_through: "tc-20", watch_started_after: "te-05", dismissed_through: "tc-20",
  unread_count: 2, unread_mentions: 1, unread_agent_comments: 1, ...over });
let runtimes;

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  runtimes = [];
});
afterEach(() => { for (const runtime of runtimes) runtime.db.close(); });

async function runtime() {
  vi.resetModules();
  const [cache, pages, tracker, watch, content] = await Promise.all([
    import("../src/core/localCache.js"), import("../src/core/trackerPages.js"),
    import("../src/core/trackerCache.js"), import("../src/core/taskWatchAuthority.js"),
    import("../src/core/taskContentAuthority.js"),
  ]);
  let db;
  const opened = await cache.transactWriteOperations("readonly", ({ records }) => { db = records.transaction.db; });
  expect(opened.committed).toBe(true);
  for (const previous of runtimes) {
    expect(db).not.toBe(previous.db); expect(cache).not.toBe(previous.cache);
    expect(pages).not.toBe(previous.pages); expect(watch).not.toBe(previous.watch);
    expect(db.name).toBe(previous.db.name);
  }
  const result = { cache, pages, tracker, watch, content, db };
  runtimes.push(result);
  return result;
}
const catalogue = (api) => api.tracker.tasksAddress(scope.deviceId, scope.projectId);
const savedRows = async (api, address = catalogue(api)) => (await api.cache.readCached(address)).value.tasks;
const pull = (api, ask, options = {}) => api.pages.pullTaskPages({
  ...scope, params, ask,
  fold: (stretch) => api.pages.foldTasksPage(catalogue(api), stretch, () => []), ...options,
});

it.each(["pending", "orphan"])("keeps the exact optimistic owner through a %s page from another connection", async (kind) => {
  const initiating = await runtime();
  expect(await pull(initiating, async () => ({ tasks: [row()] }))).toBe(true);
  const owner = await initiating.watch.beginTaskWatchMutation(scope, "task-20", { watched: false });
  expect(owner).not.toBeNull();
  if (kind === "orphan") initiating.db.close();
  const outside = await runtime();
  let pageRecord;
  expect(await pull(outside, async () => ({ tasks: [row(20, { title: "Fresh ordinary title" }), row(19)] }), {
    fold: async (stretch, page) => {
      pageRecord = page;
      expect(page.content_read.ticket.watchPending).toBe(true);
      return outside.pages.foldTasksPage(catalogue(outside), stretch, () => []);
    },
  })).toBe(true);
  const [held, discovered] = await savedRows(outside);
  expect(held).toMatchObject({ id: "task-20", title: "Fresh ordinary title", watched: false,
    __task_watch_intent: owner.id });
  expect(held[outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
  expect(discovered.id).toBe("task-19");
  expect(discovered[outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
  expect(discovered.unread_count).toBeUndefined();
  expect(pageRecord.tasks[0][outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true, eligible: false });
});

it("preserves legacy marker floors and unknown families when an older cached page is folded again", async () => {
  const api = await runtime();
  let oldStretch, oldRead;
  expect(await pull(api, async () => ({ tasks: [row()] }), {
    fold: async (stretch, page) => {
      oldStretch = stretch; oldRead = page.content_read.readOrder;
      return api.pages.foldTasksPage(catalogue(api), stretch, () => []);
    },
  })).toBe(true);
  const first = (await savedRows(api))[0];
  expect(first).toMatchObject({ read_through: "tc-20", unread_count: 2 });
  expect(first[api.watch.TASK_WATCH_CACHE].uncertain).toBe(false);

  expect(await pull(api, async () => ({ tasks: [row(20, { title: "Later page", read_through: "te-10",
    dismissed_through: "te-10", unread_count: 9, unread_mentions: 9, unread_agent_comments: 9 })] }))).toBe(true);
  const later = (await savedRows(api))[0];
  // Legacy task markers compare the suffix clock across tc/te prefixes.
  expect(later).toMatchObject({ title: "Later page", read_through: "tc-20", dismissed_through: "tc-20" });
  expect(later.unread_count).toBeUndefined();
  expect(later.unread_mentions).toBeUndefined();
  expect(later.unread_agent_comments).toBeUndefined();
  expect(later[api.watch.TASK_FAMILY_CACHE].unknown.total.observed_read_order).toBeGreaterThan(oldRead);
  const facts = structuredClone(later[api.watch.TASK_FAMILY_CACHE]);
  const lineage = structuredClone(later[api.watch.TASK_WATCH_CACHE]);

  expect(await api.pages.foldTasksPage(catalogue(api), oldStretch, () => [])).toBe(true);
  const replayed = (await savedRows(api))[0];
  expect(replayed).toMatchObject({ title: "Later page", read_through: "tc-20", dismissed_through: "tc-20" });
  expect(replayed.unread_count).toBeUndefined();
  expect(replayed[api.watch.TASK_FAMILY_CACHE]).toEqual(facts);
  expect(replayed[api.watch.TASK_WATCH_CACHE]).toEqual(lineage);
});

it("keeps a newer filtered row in the span crossed by an older empty page", async () => {
  const api = await runtime();
  const filtered = { ...params, label: "bug" };
  const address = api.tracker.tasksQueryAddress(scope.deviceId, scope.projectId, filtered);
  const selected = row(20, { labels: ["bug"] });
  let calls = 0;
  const ask = async () => {
    if (++calls === 1) {
      expect(await pull(api, async () => ({ tasks: [selected] }), {
        params: filtered, fold: (stretch) => api.pages.foldTasksPage(address, stretch, () => []),
      })).toBe(true);
      return { tasks: [], next_cursor: "below-13" };
    }
    return { tasks: [row(4, { labels: ["bug"] })] };
  };
  expect(await pull(api, ask, { params: filtered,
    fold: (stretch) => api.pages.foldTasksPage(address, stretch, () => []),
  })).toBe(true);
  expect(calls).toBe(2);
  expect((await savedRows(api, address)).map((task) => task.number)).toEqual([20, 4]);
  expect((await savedRows(api, address))[0].read_through).toBe("tc-20");
});

async function taskReplicas(api) {
  const kinds = new Set(["tracker-task", "tracker-tasks", "tracker-tasks-query", "tracker-tasks-page", "tracker-summary"]);
  const addresses = (await api.cache.cachedAddresses({ deviceId: scope.deviceId, entityId: scope.projectId }))
    .filter((address) => kinds.has(address.kind));
  const records = await Promise.all(addresses.map(async (address) => ({ address, record: await api.cache.readCached(address) })));
  return records.filter(({ record }) => record?.value?.task?.id === "task-20"
    || record?.value?.tasks?.some((task) => task.id === "task-20"));
}
const taskFrom = ({ record }) => record.value.task || record.value.tasks.find((task) => task.id === "task-20");

it("settles the transported page owner while refusing foreign and obsolete owners on the same folded replica", async () => {
  const initiating = await runtime();
  const detail = initiating.tracker.taskAddress(scope.deviceId, scope.projectId, "task-20");
  const detailRead = await initiating.content.captureTaskContentRead(detail, { revalidate: "full" });
  await initiating.tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-20",
    initiating.tracker.taskRecord(row(), []), { contentRead: detailRead });
  expect(await pull(initiating, async () => ({ tasks: [row()] }))).toBe(true);
  const ownerA = await initiating.watch.beginTaskWatchMutation(scope, "task-20", { watched: false });
  const contentRead = await initiating.content.captureTaskContentRead(scope);
  const answer = { task: row(20, { watched: false, title: "Earlier acknowledgment title" }) };
  const outside = await runtime();
  expect(await pull(outside, async () => ({ tasks: [row(20, { title: "Newer page title" })] }))).toBe(true);
  expect((await savedRows(outside))[0]).toMatchObject({ watched: false, __task_watch_intent: ownerA.id });
  const before = await taskReplicas(outside), pendingA = await outside.watch.readTaskWatchState(scope);
  const foreign = { ...ownerA, id: "foreign-owner" };
  expect(await outside.watch.settleTaskWatchMutation(foreign, { answer, contentRead })).toBe(false);
  expect(await taskReplicas(outside)).toEqual(before);
  expect(await outside.watch.readTaskWatchState(scope)).toEqual(pendingA);
  expect((await savedRows(outside))[0].__task_watch_intent).toBe(ownerA.id);

  // Committing eligible action data does not certify the unknown baseline or
  // a full canonical receipt. Inspect exact stamp cleanup and state separately.
  await outside.watch.settleTaskWatchMutation(ownerA, { answer, contentRead });
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
  expect((await savedRows(outside))[0]).toMatchObject({ watched: false, title: "Newer page title" });
  const settled = await taskReplicas(outside);
  expect(settled.some(({ address }) => address.kind === "tracker-task")).toBe(true);
  expect(settled.some(({ address }) => address.kind === "tracker-summary")).toBe(true);
  for (const copy of settled) {
    expect(taskFrom(copy).__task_watch_intent).toBeUndefined();
    expect(taskFrom(copy)[outside.watch.TASK_WATCH_CACHE]?.uncertain).toBe(false);
  }

  const ownerB = await outside.watch.beginTaskWatchMutation(scope, "task-20", { watched: true });
  expect(await pull(outside, async () => ({ tasks: [row(20, { watched: false, title: "Latest page title" })] }))).toBe(true);
  expect((await savedRows(outside))[0]).toMatchObject({ watched: true, __task_watch_intent: ownerB.id });
  const replacement = await taskReplicas(outside), pendingB = await outside.watch.readTaskWatchState(scope);
  expect(await initiating.watch.settleTaskWatchMutation(ownerA, { answer, contentRead })).toBe(false);
  expect(await taskReplicas(outside)).toEqual(replacement);
  expect(await outside.watch.readTaskWatchState(scope)).toEqual(pendingB);
  expect((await savedRows(outside))[0].__task_watch_intent).toBe(ownerB.id);
  expect(await outside.watch.settleTaskWatchMutation(ownerB, { rollback: true })).toBe(true);
  expect((await savedRows(outside))[0]).toMatchObject({ watched: false, title: "Latest page title" });
  expect((await savedRows(outside))[0].__task_watch_intent).toBeUndefined();
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
});

it("cleans a rejected owner's legitimately transported stamps from existing replicas after rollback", async () => {
  const api = await runtime();
  const current = row(20, { watched: false, title: "Current replicated title" });
  const detail = api.tracker.taskAddress(scope.deviceId, scope.projectId, current.id);
  const firstRead = await api.content.captureTaskContentRead(detail, { revalidate: "full" });
  await api.tracker.writeTaskRecord(scope.deviceId, scope.projectId, current.id,
    api.tracker.taskRecord(current, []), { contentRead: firstRead });
  const query = api.tracker.tasksQueryAddress(scope.deviceId, scope.projectId, params);
  expect(await pull(api, async () => ({ tasks: [current] }), {
    fold: async (stretch) => {
      await api.pages.foldTasksPage(query, stretch, () => []);
      return api.pages.foldTasksPage(catalogue(api), stretch, () => []);
    },
  })).toBe(true);

  // Repeating the current unwatch state supplies a real owner stamp without
  // introducing another blind watch transition during these cache-only copies.
  const owner = await api.watch.beginTaskWatchMutation(scope, current.id, { watched: false });
  const pending = await api.watch.readTaskWatchState(scope);
  const copied = (await savedRows(api))[0];
  expect(copied.__task_watch_intent).toBe(owner.id);
  const heldDetail = await api.tracker.readTaskRecord(scope.deviceId, scope.projectId, current.id);
  await api.tracker.writeTaskRecord(scope.deviceId, scope.projectId, current.id, { ...heldDetail, task: copied });
  const heldQuery = await api.tracker.readTasksQueryRecord(scope.deviceId, scope.projectId, params);
  await api.tracker.writeTasksQueryRecord(scope.deviceId, scope.projectId, params, { ...heldQuery, tasks: [copied] });
  // An actual addressed full read takes the retained detail stamp through the
  // normal detail-to-summary guard, rather than hand-writing summary metadata.
  const detailRead = await api.content.captureTaskContentRead(detail, { revalidate: "full" });
  await api.tracker.writeTaskRecord(scope.deviceId, scope.projectId, current.id,
    api.tracker.taskRecord(current, []), { contentRead: detailRead });
  expect(await api.watch.readTaskWatchState(scope)).toEqual(pending);
  const before = await taskReplicas(api);
  const stampedKinds = before.filter((copy) => taskFrom(copy).__task_watch_intent === owner.id).map(({ address }) => address.kind);
  expect(stampedKinds).toEqual(expect.arrayContaining(["tracker-task", "tracker-summary", "tracker-tasks", "tracker-tasks-query"]));
  expect(before.some(({ address }) => address.kind === "tracker-tasks-page")).toBe(true);
  for (const copy of before) expect(taskFrom(copy).title).toBe(current.title);

  expect(await api.watch.settleTaskWatchMutation(owner, { rollback: true })).toBe(true);
  expect(await api.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
  const after = await taskReplicas(api);
  expect(after.map(({ address }) => address)).toEqual(before.map(({ address }) => address));
  for (const copy of after) {
    expect(taskFrom(copy)).toMatchObject({ title: current.title, watched: false });
    expect(taskFrom(copy).__task_watch_intent).toBeUndefined();
  }
});

const deferred = () => {
  let resolve;
  const promise = new Promise((yes) => { resolve = yes; });
  return { promise, resolve };
};

it("retires a terminal stamp copied by another connection after actual replica address discovery", async () => {
  const first = await runtime();
  const actions = await import("../src/core/taskWaitingOnCache.js");
  const current = row(20, { watched: false, title: "Prepared cached row" });
  expect(await pull(first, async () => ({ tasks: [current] }))).toBe(true);
  const owner = await first.watch.beginTaskWatchMutation(scope, current.id, { watched: false });
  const prepared = structuredClone((await savedRows(first))[0]);
  expect(prepared.__task_watch_intent).toBe(owner.id);
  const contentRead = await first.content.captureTaskContentRead(scope);
  const outside = await runtime();
  const queryParams = { ...params, state: "open" };
  const target = outside.tracker.tasksQueryAddress(scope.deviceId, scope.projectId, queryParams);
  expect(await outside.cache.readCached(target)).toBeUndefined();
  const discovered = deferred(), resume = deferred();
  const actualAddresses = first.cache.cachedAddresses;
  const discovery = vi.spyOn(first.cache, "cachedAddresses").mockImplementationOnce(async (prefix) => {
    const addresses = await actualAddresses(prefix);
    discovered.resolve({ prefix, addresses });
    await resume.promise;
    return addresses;
  });
  let settled;
  try {
    settled = actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task: current },
      { contentRead, issuedRead: contentRead.readOrder, watchOwner: owner });
    const captured = await discovered.promise;
    expect(captured.prefix).toEqual({ deviceId: scope.deviceId, entityId: scope.projectId });
    expect(captured.addresses.some((address) => address.kind === target.kind && address.sub === target.sub)).toBe(false);
    // This actual public writer commits on B's independent IDB connection
    // after A's address result was read, while A has not settled the owner.
    await outside.tracker.writeTasksQueryRecord(scope.deviceId, scope.projectId, queryParams,
      outside.tracker.tasksRecord([prepared], []));
    expect((await savedRows(outside, target))[0].__task_watch_intent).toBe(owner.id);
    expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true });
    resume.resolve();
    await settled;
    expect(discovery).toHaveBeenCalledTimes(1);
    const terminal = await outside.watch.readTaskWatchState(scope);
    expect(terminal).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });

    // A future admitted writer must consult terminal ownership in its own
    // transaction. A's finite address list cannot have cleaned this late copy.
    const refresh = await outside.content.captureTaskContentRead(target, { revalidate: "full" });
    await outside.tracker.writeTasksQueryRecord(scope.deviceId, scope.projectId, queryParams,
      outside.tracker.tasksRecord([row(20, { watched: false, title: "Later admitted query title" })], []),
      { contentRead: refresh });
    expect(await outside.watch.readTaskWatchState(scope)).toEqual(terminal);
    const held = (await savedRows(outside, target))[0];
    expect(held).toMatchObject({ title: "Later admitted query title", watched: false });
    expect(held.unread_count).toBeUndefined();
    expect(held[outside.watch.TASK_WATCH_CACHE]?.uncertain).toBe(false);
    expect(held.__task_watch_intent).toBeUndefined();
  } finally {
    resume.resolve();
    if (settled) await settled;
    discovery.mockRestore();
  }
});

it("retires only older copied protection while a newer metadata-only owner and the old owner remain pending", async () => {
  const first = await runtime();
  expect(await pull(first, async () => ({ tasks: [row()] }))).toBe(true);
  const older = await first.watch.beginTaskWatchMutation(scope, "task-20", { watched: false });
  const prepared = structuredClone((await savedRows(first))[0]);
  expect(prepared).toMatchObject({ watched: false, __task_watch_intent: older.id });
  const outside = await runtime();
  const newer = await outside.watch.beginTaskWatchMutation(scope, "task-20", { watched: true, metadataOnly: true });
  expect(newer.intentRead).toBeGreaterThan(older.intentRead);
  const pendingBoth = await outside.watch.readTaskWatchState(scope);
  expect(pendingBoth).toMatchObject({ pendingCount: 2, uncertain: true, eligible: false });
  // Metadata-only registration is a barrier, not optimistic cache paint.
  expect((await savedRows(outside))[0].watched).toBe(false);
  expect((await savedRows(outside))[0].__task_watch_intent).not.toBe(newer.id);

  const copyToQuery = async () => {
    const queryParams = { ...params, state: "open" };
    const address = outside.tracker.tasksQueryAddress(scope.deviceId, scope.projectId, queryParams);
    await outside.tracker.writeTasksQueryRecord(scope.deviceId, scope.projectId, queryParams,
      outside.tracker.tasksRecord([prepared], []));
    return { address, task: (await savedRows(outside, address))[0] };
  };
  const pendingCopy = await copyToQuery();
  expect(await outside.watch.readTaskWatchState(scope)).toEqual(pendingBoth);
  expect(pendingCopy.task).toMatchObject({ id: "task-20", watched: false, title: prepared.title });
  expect(pendingCopy.task.__task_watch_intent).toBeUndefined();
  expect(pendingCopy.task[outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
  expect(pendingCopy.task.unread_count).toBeUndefined();
  expect((await outside.content.captureTaskContentRead(scope)).ticket).toMatchObject({ watchPending: true, watchUncertain: true });

  // Settling B cannot settle A. The newer terminal note still supersedes A's
  // copied protection without replaying the original A rollback to watched=true.
  expect(await outside.watch.settleTaskWatchMutation(newer, { rollback: true })).toBe(true);
  const pendingOlder = await outside.watch.readTaskWatchState(scope);
  expect(pendingOlder).toMatchObject({ pendingCount: 1, uncertain: true, eligible: false });
  const terminalCopy = await copyToQuery();
  expect(await outside.watch.readTaskWatchState(scope)).toEqual(pendingOlder);
  expect(terminalCopy.task).toMatchObject({ watched: false, title: prepared.title });
  expect(terminalCopy.task.__task_watch_intent).toBeUndefined();
  expect(terminalCopy.task[outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
  expect((await outside.content.captureTaskContentRead(scope)).ticket.watchPending).toBe(true);

  expect(await first.watch.settleTaskWatchMutation(older, { rollback: true })).toBe(false);
  const completed = await outside.watch.readTaskWatchState(scope);
  expect(completed).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
  expect((await savedRows(outside))[0].watched).toBe(false);
  const lateCopy = await copyToQuery();
  expect(lateCopy.task).toMatchObject({ watched: false, title: prepared.title });
  expect(lateCopy.task.__task_watch_intent).toBeUndefined();
  expect(await outside.watch.readTaskWatchState(scope)).toEqual(completed);
  expect(await first.watch.settleTaskWatchMutation(older, { rollback: true })).toBe(false);
  expect(await outside.watch.readTaskWatchState(scope)).toEqual(completed);
});

it("keeps the terminal intent after a full acknowledgment updates populated detail and rejects a late stamped copy", async () => {
  const first = await runtime();
  const order = await import("../src/core/taskReadOrder.js");
  const detail = first.tracker.taskAddress(scope.deviceId, scope.projectId, "task-20");
  const seededRead = await first.content.captureTaskContentRead(detail, { revalidate: "full" });
  await first.tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-20",
    first.tracker.taskRecord(row(), []), { contentRead: seededRead });
  expect((await first.tracker.readTaskRecord(scope.deviceId, scope.projectId, "task-20")).task.body).toBe(row().body);
  expect(await pull(first, async () => ({ tasks: [row()] }))).toBe(true);
  const owner = await first.watch.beginTaskWatchMutation(scope, "task-20", { watched: false });
  const prepared = structuredClone((await savedRows(first))[0]);
  expect(prepared.__task_watch_intent).toBe(owner.id);
  const contentRead = await first.content.captureTaskContentRead(detail, { revalidate: "full" });
  const outside = await runtime();
  const answer = row(20, { watched: false, title: "Acknowledged populated detail", body: "Acknowledged full body" });
  await outside.watch.settleTaskWatchMutation(owner, { answer: { task: answer }, contentRead });
  const populated = await outside.tracker.readTaskRecord(scope.deviceId, scope.projectId, "task-20");
  expect(populated.task).toMatchObject({ title: answer.title, body: answer.body });
  const terminal = await outside.watch.readTaskWatchState(scope);
  expect(terminal).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
  const note = (await outside.cache.readCached(order.taskWatchIntentAddress(scope, "task-20"))).value;
  expect(note.latest_watch_intent).toMatchObject({ ownerId: owner.id, intentRead: owner.intentRead, disposition: "acknowledged" });

  const queryParams = { ...params, state: "open" };
  const query = outside.tracker.tasksQueryAddress(scope.deviceId, scope.projectId, queryParams);
  expect(await outside.cache.readCached(query)).toBeUndefined();
  await outside.tracker.writeTasksQueryRecord(scope.deviceId, scope.projectId, queryParams,
    outside.tracker.tasksRecord([prepared], []));
  const copied = (await savedRows(outside, query))[0];
  expect(copied.watched).toBe(false);
  expect(copied.__task_watch_intent).toBeUndefined();
  expect(copied[outside.watch.TASK_WATCH_CACHE]?.uncertain).not.toBe(false);
  expect(await outside.watch.readTaskWatchState(scope)).toEqual(terminal);
});

it("does not resurrect rolled-back unwatch paint by reclassifying an obsolete copied stamp as a blind edit", async () => {
  const first = await runtime();
  expect(await pull(first, async () => ({ tasks: [row()] }))).toBe(true);
  const owner = await first.watch.beginTaskWatchMutation(scope, "task-20", { watched: false });
  const prepared = structuredClone((await savedRows(first))[0]);
  expect(prepared).toMatchObject({ watched: false, __task_watch_intent: owner.id });
  const outside = await runtime();
  expect(await outside.watch.settleTaskWatchMutation(owner, { rollback: true })).toBe(true);
  const restored = (await savedRows(outside))[0];
  expect(restored.watched).toBe(true);
  expect(restored.__task_watch_intent).toBeUndefined();
  const terminal = await outside.watch.readTaskWatchState(scope);
  const controlAddress = outside.watch.taskWatchControlAddress(scope);
  const beforeControl = (await outside.cache.readCached(controlAddress)).value;
  expect(terminal).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });

  // The incoming bytes are the actual failed optimistic cache copy. Removing
  // its obsolete tag cannot turn those same bytes into a new user mutation.
  await outside.tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    outside.tracker.tasksRecord([prepared], []));
  const held = (await savedRows(outside))[0];
  expect(held).toMatchObject({ watched: true, title: restored.title });
  expect(held.__task_watch_intent).toBeUndefined();
  expect(held[outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
  expect(held.unread_count).toBeUndefined();
  expect(await outside.watch.readTaskWatchState(scope)).toEqual(terminal);
  expect((await outside.cache.readCached(controlAddress)).value.blindRetirementEpoch).toBe(beforeControl.blindRetirementEpoch);
});

it("applies a metadata-only owner's full watch answer after unrelated owned changes to the admission epoch", async () => {
  const first = await runtime();
  const order = await import("../src/core/taskReadOrder.js");
  expect(await pull(first, async () => ({ tasks: [row(20, { watched: false }), row(19, { watched: false })] }))).toBe(true);
  const ownerB = await first.watch.beginTaskWatchMutation(scope, "task-20", { watched: true, metadataOnly: true });
  const contentRead = await first.content.captureTaskContentRead(scope);
  const controlAddress = first.watch.taskWatchControlAddress(scope);
  const beforeControl = (await first.cache.readCached(controlAddress)).value;
  const noteAddress = order.taskWatchIntentAddress(scope, "task-20");
  const beforeNote = (await first.cache.readCached(noteAddress)).value.latest_watch_intent;
  expect((await savedRows(first)).find((task) => task.id === "task-20").watched).toBe(false);

  const outside = await runtime();
  const unrelated = await outside.watch.beginTaskWatchMutation(scope, "task-19", { watched: true });
  expect(await outside.watch.settleTaskWatchMutation(unrelated, { rollback: true })).toBe(true);
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true });
  // Keep a second actual owner live so B's settlement must preserve its count
  // and painted row, not merely finish at an uninformative empty owner set.
  const remaining = await outside.watch.beginTaskWatchMutation(scope, "task-19", { watched: true });
  const unrelatedRow = structuredClone((await savedRows(outside)).find((task) => task.id === "task-19"));
  const duringControl = (await outside.cache.readCached(controlAddress)).value;
  const currentRead = await outside.content.captureTaskContentRead(scope);
  expect(duringControl.watchRetirementEpoch).not.toBe(contentRead.ticket.watchRetirementEpoch);
  expect(duringControl.blindRetirementEpoch).toBe(beforeControl.blindRetirementEpoch);
  expect(duringControl.generation).toBe(beforeControl.generation);
  expect(duringControl.pendingGeneration).toBe(beforeControl.pendingGeneration);
  expect(currentRead.ticket.storeGeneration).toBe(contentRead.ticket.storeGeneration);
  expect(currentRead.ticket.scopeGeneration).toBe(contentRead.ticket.scopeGeneration);
  expect((await outside.cache.readCached(noteAddress)).value.latest_watch_intent).toEqual(beforeNote);
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 2, uncertain: true, eligible: false });

  await outside.watch.settleTaskWatchMutation(ownerB, { contentRead,
    answer: { task: row(20, { watched: true, title: "Successful metadata-only watch answer" }) } });
  const tasks = await savedRows(outside);
  expect(tasks.find((task) => task.id === "task-19")).toEqual(unrelatedRow);
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true, eligible: false });
  const answered = tasks.find((task) => task.id === "task-20");
  expect(answered.title).toBe("Successful metadata-only watch answer");
  expect(answered.__task_watch_intent).toBeUndefined();
  expect((await outside.cache.readCached(noteAddress)).value.latest_watch_intent).toMatchObject({ ownerId: ownerB.id, disposition: "acknowledged" });
  // Desired paint is separate from server-family/canonical receipt certainty.
  expect(answered.watched).toBe(true);
  expect(answered[outside.watch.TASK_WATCH_CACHE]?.uncertain).not.toBe(false);
  expect(await outside.watch.settleTaskWatchMutation(remaining, { rollback: true })).toBe(true);
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
});

async function catalogueOnlyFalse(api) {
  const address = catalogue(api);
  const contentRead = await api.content.captureTaskContentRead(address, { revalidate: "full" });
  await api.tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    api.tracker.tasksRecord([row(20, { watched: false })], []), { contentRead });
  const summaries = await import("../src/core/taskSummaryCache.js");
  const detail = api.tracker.taskAddress(scope.deviceId, scope.projectId, "task-20");
  expect(await api.cache.readCached(detail)).toBeUndefined();
  expect(await api.cache.readCached(summaries.summaryAddress(scope, "task-20"))).toBeUndefined();
  return { detail, task: (await savedRows(api))[0] };
}

it("protects a conflicting unowned detail creation from an earlier metadata-only watch acknowledgment", async () => {
  const first = await runtime();
  const { detail } = await catalogueOnlyFalse(first);
  const owner = await first.watch.beginTaskWatchMutation(scope, "task-20", { watched: true, metadataOnly: true });
  const contentRead = await first.content.captureTaskContentRead(scope);
  const controlAddress = first.watch.taskWatchControlAddress(scope);
  const before = (await first.cache.readCached(controlAddress)).value;
  const outside = await runtime();
  await outside.tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-20",
    outside.tracker.taskRecord(row(20, { title: "Unowned new detail", watched: true, watch_started_after: "te-blind" }), []));
  const created = (await outside.cache.readCached(detail)).value.task;
  expect(created).toMatchObject({ watched: true, watch_started_after: "te-blind" });
  expect(created[outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true });
  const afterCreation = (await outside.cache.readCached(controlAddress)).value;

  await outside.watch.settleTaskWatchMutation(owner, { contentRead,
    answer: { task: row(20, { watched: true, watch_started_after: "te-ack" }) } });
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
  const held = (await outside.cache.readCached(detail)).value.task;
  expect(held).toMatchObject({ watched: true, watch_started_after: "te-blind" });
  expect(held[outside.watch.TASK_WATCH_CACHE]?.source).not.toBe("ack");
  expect(afterCreation.blindRetirementEpoch).not.toBe(before.blindRetirementEpoch);
});

it("does not treat an unchanged genuinely observed catalogue copy into absent detail as blind interference", async () => {
  const first = await runtime();
  const { detail, task: observed } = await catalogueOnlyFalse(first);
  expect(observed[first.watch.TASK_WATCH_CACHE]).toMatchObject({ taskId: "task-20", source: "read", uncertain: false });
  expect(observed.__task_content_cache.header.read_order).toBeGreaterThan(0);
  const prepared = structuredClone(observed);
  const owner = await first.watch.beginTaskWatchMutation(scope, "task-20", { watched: true, metadataOnly: true });
  const contentRead = await first.content.captureTaskContentRead(scope);
  const controlAddress = first.watch.taskWatchControlAddress(scope);
  const before = (await first.cache.readCached(controlAddress)).value;
  const outside = await runtime();
  await outside.tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-20", outside.tracker.taskRecord(prepared, []));
  expect((await outside.cache.readCached(detail)).value.task.watched).toBe(false);
  expect((await outside.cache.readCached(controlAddress)).value.blindRetirementEpoch).toBe(before.blindRetirementEpoch);
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true });

  await outside.watch.settleTaskWatchMutation(owner, { contentRead,
    answer: { task: row(20, { watched: true, watch_started_after: "te-ack" }) } });
  const held = (await outside.cache.readCached(detail)).value.task;
  expect(held).toMatchObject({ watched: true, watch_started_after: "te-ack" });
  expect(held[outside.watch.TASK_WATCH_CACHE]?.uncertain).toBe(false);
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
});

async function actualColdWatchRead({ pending = false, held = false, metadataOnly = false } = {}) {
  const first = await runtime();
  const summaries = await import("../src/core/taskSummaryCache.js");
  const detail = first.tracker.taskAddress(scope.deviceId, scope.projectId, "task-20");
  const normalized = summaries.summaryAddress(scope, "task-20");
  const original = row(20, { watched: true, watch_started_after: "te-held" });
  const catalogueRead = await first.content.captureTaskContentRead(catalogue(first), { revalidate: "full" });
  await first.tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    first.tracker.tasksRecord([original], []), { contentRead: catalogueRead });
  expect(await first.cache.readCached(detail)).toBeUndefined();
  expect(await first.cache.readCached(normalized)).toBeUndefined();
  if (held) {
    const heldRead = await first.content.captureTaskContentRead(detail, { revalidate: "full" });
    await first.tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-20",
      first.tracker.taskRecord(original, []), { contentRead: heldRead });
  }
  const owner = pending ? await first.watch.beginTaskWatchMutation(scope, "task-20", { watched: false, metadataOnly }) : null;
  if (pending) expect(owner).not.toBeNull();
  const catalogueBefore = await first.cache.readCached(catalogue(first));
  const before = await first.watch.readTaskWatchState(scope);
  expect(before).toMatchObject({ pendingCount: pending ? 1 : 0, uncertain: pending, eligible: !pending });
  const outside = await runtime();
  const contentRead = await outside.content.captureTaskContentRead(detail, { revalidate: "full" });
  expect(contentRead.ticket.watchPending).toBe(pending);
  const raw = row(20, { title: "Admitted raw title", body: "Admitted raw body", watched: true,
    watch_started_after: "te-refused", read_through: "tc-90", dismissed_through: "te-90",
    review_summary: { task_id: "task-20", version: 2, status: "open", mode: "pull_request" } });
  await outside.tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-20",
    outside.tracker.taskRecord(raw, []), { contentRead });
  const detailValue = (await outside.cache.readCached(detail)).value;
  const summaryValue = (await outside.cache.readCached(normalized)).value;
  expect(detailValue.task.body).toBe(raw.body);
  for (const task of [detailValue.task, summaryValue.task]) {
    expect(task).toMatchObject({ title: raw.title, read_through: raw.read_through,
      dismissed_through: raw.dismissed_through, review_summary: raw.review_summary });
  }
  expect(await outside.cache.readCached(catalogue(outside))).toEqual(catalogueBefore);
  expect(await outside.watch.readTaskWatchState(scope)).toEqual(before);
  return { outside, owner, catalogueBefore, detail: detailValue.task, summary: summaryValue };
}

it("does not paint a cold detail or normalized row from a raw watch reply while a metadata-only owner is pending", async () => {
  const result = await actualColdWatchRead({ pending: true, metadataOnly: true });
  // The pending request desires false but never paints the cache. The old
  // catalogue true is not a held detail/summary or an observed raw response.
  expect(result.catalogueBefore.value.tasks[0].watched).toBe(true);
  expect(result.catalogueBefore.value.tasks[0].__task_watch_intent).toBeUndefined();
  for (const task of [result.detail, result.summary.task]) {
    expect(task).not.toHaveProperty("watched");
    expect(task).not.toHaveProperty("watch_started_after");
    expect(task[result.outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
  }
  expect(result.summary).toMatchObject({ watch_uncertain: true, needs_unread_refresh: true });
});

it("admits authoritative raw watch paint on a cold genuine read without a pending owner", async () => {
  const result = await actualColdWatchRead();
  for (const task of [result.detail, result.summary.task]) {
    expect(task).toMatchObject({ watched: true, watch_started_after: "te-refused" });
    expect(task[result.outside.watch.TASK_WATCH_CACHE]).toMatchObject({ source: "read", uncertain: false });
  }
  expect(result.summary).toMatchObject({ watch_uncertain: false, needs_unread_refresh: false });
});

it("retains actual held detail paint while rejecting a pending raw watch lineage", async () => {
  const result = await actualColdWatchRead({ pending: true, held: true });
  expect(result.catalogueBefore.value.tasks[0]).toMatchObject({ watched: false, __task_watch_intent: result.owner.id });
  for (const task of [result.detail, result.summary.task]) {
    expect(task).toMatchObject({ watched: true, watch_started_after: "te-held" });
    expect(task[result.outside.watch.TASK_WATCH_CACHE]?.uncertain).not.toBe(false);
  }
  expect(result.summary).toMatchObject({ watch_uncertain: true, needs_unread_refresh: true });
});

it("protects an untagged local false lineage created in an existing lineageless detail from an earlier full acknowledgment", async () => {
  const first = await runtime();
  const { detail } = await catalogueOnlyFalse(first);
  const owner = await first.watch.beginTaskWatchMutation(scope, "task-20", { watched: true, metadataOnly: true });
  const earlier = await first.content.captureTaskContentRead(scope);
  const outside = await runtime();
  const pendingRead = await outside.content.captureTaskContentRead(detail, { revalidate: "full" });
  expect(pendingRead.ticket.watchPending).toBe(true);
  await outside.tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-20",
    outside.tracker.taskRecord(row(20, { title: "Current ordinary bytes", watched: true, watch_started_after: "te-refused" }), []),
    { contentRead: pendingRead });
  const lineageless = (await outside.cache.readCached(detail)).value;
  expect(lineageless.task).not.toHaveProperty("watched");
  expect(lineageless.task).not.toHaveProperty("watch_started_after");
  const controlAddress = outside.watch.taskWatchControlAddress(scope);
  const beforeLocal = (await outside.cache.readCached(controlAddress)).value;

  // Deliberately untagged local write, not a provenance-preserving cache copy:
  // strip private metadata from actual admitted bytes before choosing false.
  const local = Object.fromEntries(Object.entries(lineageless.task).filter(([key]) => !key.startsWith("__")));
  local.watched = false;
  expect(local).not.toHaveProperty("watch_started_after");
  await outside.tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-20", { ...lineageless, task: local });
  const written = (await outside.cache.readCached(detail)).value.task;
  expect(written).toMatchObject({ title: lineageless.task.title, body: lineageless.task.body, watched: false });
  expect(written).not.toHaveProperty("watch_started_after");
  expect(written[outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
  const afterLocal = (await outside.cache.readCached(controlAddress)).value;
  const retiredBeforeAck = afterLocal.blindRetirementEpoch !== beforeLocal.blindRetirementEpoch;
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true });

  await outside.watch.settleTaskWatchMutation(owner, { contentRead: earlier,
    answer: { task: row(20, { watched: true, watch_started_after: "te-ack" }) } });
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
  const held = (await outside.cache.readCached(detail)).value.task;
  expect(held.watched, `blind fence retired before acknowledgment: ${retiredBeforeAck}`).toBe(false);
  expect(held).not.toHaveProperty("watch_started_after");
  expect(held[outside.watch.TASK_WATCH_CACHE]?.source).not.toBe("ack");
  expect(retiredBeforeAck).toBe(true);
});
