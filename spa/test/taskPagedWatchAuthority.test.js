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
  expect(first[api.watch.TASK_WATCH_CACHE].uncertain).toBe(true);

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
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
  expect((await savedRows(outside))[0]).toMatchObject({ watched: false, title: "Newer page title" });
  const settled = await taskReplicas(outside);
  expect(settled.some(({ address }) => address.kind === "tracker-task")).toBe(true);
  expect(settled.some(({ address }) => address.kind === "tracker-summary")).toBe(true);
  for (const copy of settled) {
    expect(taskFrom(copy).__task_watch_intent).toBeUndefined();
    expect(taskFrom(copy)[outside.watch.TASK_WATCH_CACHE]?.uncertain).not.toBe(false);
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
  expect(await outside.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
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
  expect(await api.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
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
    expect(terminal).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });

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
    expect(held[outside.watch.TASK_WATCH_CACHE]?.uncertain).not.toBe(false);
    expect(held.__task_watch_intent).toBeUndefined();
  } finally {
    resume.resolve();
    if (settled) await settled;
    discovery.mockRestore();
  }
});
