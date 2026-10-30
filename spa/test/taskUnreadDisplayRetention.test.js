// @vitest-environment jsdom
// D5 case 7: retain what each real consumer actually selected from its cached
// list before an incomplete full action invalidates the counter families.
// Original authority suites stay unchanged. No display state or proof is seeded.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { columns, comment, task as wireTask } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] }, tasks: {} }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const scope = { deviceId: "d5-rendered-device", projectId: "proj-1" };
const taskId = "d5-rendered-task";
const project = { id: scope.projectId, project_id: scope.projectId, deviceId: scope.deviceId,
  projectKey: `${scope.deviceId}|${scope.projectId}`, name: "Build" };
const filters = { states: ["open"] };
const WAIT = { timeout: 3000, interval: 10 };
const full = (extra = {}) => wireTask({ id: taskId, project_id: scope.projectId, number: 17,
  title: "Before incomplete action", status: "ready", assignee: { kind: "user" }, watched: true,
  read_through: "tc-10", watch_started_after: "te-15", dismissed_through: "te-05",
  unread_count: 3, unread_mentions: 1, unread_agent_comments: 2,
  updated_at: "2026-10-29T12:00:00Z", attachments: [], ...extra });
const summaryOf = ({ body: _body, attachments, ...task }) => ({ ...task, attachment_count: attachments.length });

let cache, tracker, summaries, actions, content, pane, follower, paneHost, inboxHost, rpc, shell, taskFeed;
const value = async (address) => (await cache.readCached(address))?.value;
const listedTask = async () => (await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0];

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="d5-pane"></div><div id="d5-inbox"></div>';
  paneHost = document.querySelector("#d5-pane");
  inboxHost = document.querySelector("#d5-inbox");
  pane = null; follower = null; shell = null; taskFeed = null;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  actions = await import("../src/core/taskWaitingOnCache.js");
  content = await import("../src/core/taskContentAuthority.js");
  // Wire refreshes cannot contribute either the initial or retained display.
  rpc = vi.fn(() => new Promise(() => {}));
});

afterEach(() => {
  pane?.dispose();
  follower?.dispose();
  shell?.unmountInboxList();
  taskFeed?.stopFeed();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

async function seedSelectedInputs() {
  const task = full();
  // An action updates an already admitted detail; its scope-wide ticket is
  // not an addressed full-detail read that can populate an absent body.
  // Preserve the original null-detail harness failure separately. This older
  // real detail keeps the detail-replica safety oracle without owning the
  // consumers' initial count: the newer list must still supply their 3.
  const detailAddress = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
  const detailRead = await content.captureTaskContentRead(detailAddress, { revalidate: "full" });
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
    tracker.taskRecord(full({ title: "Older cached detail", updated_at: "2026-10-29T11:59:00Z" }), []),
    { contentRead: detailRead })).toBe(true);
  const summaryRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const ticket = await summaries.beginSummaryQuery(scope, filters, summaryRead.readOrder, { contentRead: summaryRead });
  expect(await summaries.landSummaryPage(ticket, { summary: true, tasks: [summaryOf(task)],
    identities: {}, basis: "d5-before" }, { read: summaryRead.readOrder })).toBe(true);
  const address = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const listRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord([task], columns(), listRead.readOrder), { contentRead: listRead })).toBe(true);
  const listed = await listedTask();
  const detailed = await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  const { changedSince } = await import("../src/core/trackerModel.js");
  const { taskUnreadCount } = await import("../src/core/taskUnread.js");
  expect(listed.unread_count).toBe(3);
  expect(detailed.task).toMatchObject({ title: "Older cached detail", updated_at: "2026-10-29T11:59:00Z" });
  expect(detailed.timeline).toEqual([]);
  expect(changedSince(listed, detailed)).toBe(true);
  expect(taskUnreadCount(listed, detailed)).toBe(3);
}

async function invalidateWithActualIncompleteAction() {
  const task = full({ title: "After incomplete action", read_through: "tc-20", updated_at: "2026-10-29T12:01:00Z" });
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete task[field];
  const contentRead = await content.captureTaskContentRead(scope);
  expect(await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task },
    { contentRead, issuedRead: contentRead.readOrder })).toBe(true);
  const normalized = await value(summaries.summaryAddress(scope, taskId));
  const detailed = await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  expect(normalized.needs_unread_refresh).toBe(true);
  for (const row of [normalized.task, await listedTask(), detailed.task]) {
    expect(row).toMatchObject({ title: "After incomplete action", read_through: "tc-20", watch_started_after: "te-15" });
    for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) expect(row).not.toHaveProperty(field);
    expect(row.__task_family_cache?.total).toBeUndefined();
    expect(row.__task_family_cache?.unknown?.total?.observed_read_order).toEqual(expect.any(Number));
    expect(row.__task_family_cache.unknown.total).not.toHaveProperty("values");
  }
}

async function mountPane() {
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(paneHost, { deviceId: scope.deviceId, projectId: scope.projectId,
    projectKey: project.projectKey, projectName: project.name, defaultView: "list", callRpc: rpc,
    catalog: () => ({ providers: [] }), feed: () => ({ items: [], workspaces: [] }) });
  await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3"), WAIT);
}

async function mountInboxComponent() {
  for (const [kind, held] of [["projects", [project]], ["workspaces", []],
    ["feed", { projects: [project], workspaces: [], runs: [], items: [] }]]) {
    await cache.writeCached({ deviceId: scope.deviceId, entityId: "", kind }, held);
  }
  const { followWatchedTasks } = await import("../src/core/watchedTaskFollower.js");
  const { inboxRowHtml } = await import("../src/core/inbox.js");
  follower = followWatchedTasks({ onChange: () => { inboxHost.innerHTML = follower.entries().map((entry) => inboxRowHtml(entry)).join(""); } });
  follower.follow([project]);
  await vi.waitFor(() => expect(follower.entries()[0]?.unreadCount).toBe(3), WAIT);
  await vi.waitFor(() => expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull(), WAIT);
}

describe("D5 actual consumer selection, original case 7 nonzero total", () => {
  it("retains the mounted list's previously displayed 3 when an incomplete full action makes total authority UNKNOWN", async () => {
    await seedSelectedInputs();
    await mountPane();
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(paneHost.querySelector(".task-title")?.textContent).toBe("After incomplete action"), WAIT);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
  });

  it("retains the followed Inbox row's previously displayed 3 when an incomplete full action makes total authority UNKNOWN", async () => {
    await seedSelectedInputs();
    await mountInboxComponent();
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(inboxHost.querySelector(".stitle")?.textContent).toContain("After incomplete action"), WAIT);
    expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull();
    expect(follower.entries()[0].unreadCount).toBe(3);
  });
});

async function mountInboxProducerComponent() {
  for (const [kind, held] of [["projects", [project]], ["workspaces", []],
    ["feed", { projects: [project], workspaces: [], runs: [], items: [] }]]) {
    await cache.writeCached({ deviceId: scope.deviceId, entityId: "", kind }, held);
  }
  const { followWatchedTasks } = await import("../src/core/watchedTaskFollower.js");
  const { inboxRowHtml } = await import("../src/core/inbox.js");
  const render = (entry) => typeof follower.renderEntry === "function"
    ? follower.renderEntry(entry, () => inboxRowHtml(entry)) : inboxRowHtml(entry);
  follower = followWatchedTasks({ onChange: () => { inboxHost.innerHTML = follower.entries().map(render).join(""); } });
  follower.follow([project]);
  await vi.waitFor(() => expect(follower.entries()[0]?.unreadCount).toBe(3), WAIT);
  await vi.waitFor(() => expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull(), WAIT);
}

async function publishCoveringTotal(total) {
  const task = full({ title: `Covering total ${total}`, read_through: "tc-20", unread_count: total,
    unread_mentions: 0, unread_agent_comments: 0, updated_at: `2026-10-29T12:0${total === 0 ? 2 : 3}:00Z` });
  const address = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord([task], columns(), contentRead.readOrder), { contentRead })).toBe(true);
  expect((await listedTask()).unread_count).toBe(total);
}

describe("D5 additional actual-owner controls before persistence and lineage implementation", () => {
  it("retains actual Pane display through an unobserved blind floor rewrite without treating it as a new watch incarnation", async () => {
    await seedSelectedInputs(); await mountPane();
    const task = { ...await listedTask(), title: "Blind floor rewrite", watch_started_after: "te-90" };
    for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete task[field];
    expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([task], columns()))).toBe(true);
    await vi.waitFor(() => expect(paneHost.querySelector(".task-title")?.textContent).toBe("Blind floor rewrite"), WAIT);
    expect((await listedTask()).watch_started_after).toBe("te-90");
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
  });

  it("keeps mounted Pane display and canonical cache when its bounded display-note persistence hits quota", async () => {
    await seedSelectedInputs();
    const { taskWatchIntentAddress } = await import("../src/core/taskReadOrder.js");
    const noteAddress = taskWatchIntentAddress(scope, taskId);
    const prior = await value(noteAddress);
    let quotaWrites = 0;
    const put = IDBObjectStore.prototype.put;
    vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
      if (record?.value?.task_unread_display?.["tasks-pane"]) {
        quotaWrites += 1; throw new DOMException("Display note quota refused", "QuotaExceededError");
      }
      return put.call(this, record, key);
    });
    await mountPane();
    await vi.waitFor(() => expect(quotaWrites).toBeGreaterThan(0), WAIT);
    expect(await value(noteAddress)).toEqual(prior);
    expect((await listedTask()).unread_count).toBe(3);
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(paneHost.querySelector(".task-title")?.textContent).toBe("After incomplete action"), WAIT);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    expect(paneHost.querySelector(".error, .warn:not([hidden])")).toBeNull();
  });

  it("freezes the actual cached timeline selection 2 rather than listed 3 and accepts later covering zero and value", async () => {
    await seedSelectedInputs(); await mountPane();
    const address = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
    const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
    const timeline = ["tc-16", "tc-17"].map((id) => comment({ id, task_id: taskId,
      author: { kind: "agent", agent_id: "agent" } }));
    expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
      tracker.taskRecord(full(), timeline), { contentRead })).toBe(true);
    expect((await listedTask()).unread_count).toBe(3);
    await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")?.textContent).toBe("2"), WAIT);
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(paneHost.querySelector(".task-title")?.textContent).toBe("After incomplete action"), WAIT);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("2");
    await publishCoveringTotal(0);
    await vi.waitFor(() => expect(paneHost.querySelector(".task-title")?.textContent).toBe("Covering total 0"), WAIT);
    expect(paneHost.querySelector(".task-unread")).toBeNull();
    await publishCoveringTotal(5);
    await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")?.textContent).toBe("5"), WAIT);
  });

  it("producer-hookup variant retains actual Inbox renderEntry selection without changing the original component oracle", async () => {
    await seedSelectedInputs(); await mountInboxProducerComponent();
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(inboxHost.querySelector(".stitle")?.textContent).toContain("After incomplete action"), WAIT);
    expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull();
    expect(follower.entries()[0].unreadCount).toBe(3);
  });

  it("does not seed Inbox display from a never-rendered closed Recent row in the actual mounted shell", async () => {
    vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(new Date("2026-10-31T12:00:00Z"));
    await seedSelectedInputs();
    document.body.innerHTML = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
    const { App } = await import("../src/app.js");
    Object.assign(App, { route: { name: "inbox" }, devices: [{ id: scope.deviceId, name: "D5 device", status: "online" }],
      selectedDeviceId: scope.deviceId, deviceFilter: null });
    for (const [kind, held] of [["projects", [project]], ["workspaces", []],
      ["feed", { projects: [project], workspaces: [], runs: [], items: [] }]]) {
      await cache.writeCached({ deviceId: scope.deviceId, entityId: "", kind }, held);
    }
    shell = await import("../src/core/inboxView.js"); taskFeed = await import("../src/core/taskFeed.js");
    shell.setInboxView("inbox"); shell.mountInboxList(); await taskFeed.startFeed();
    await vi.waitFor(() => expect(document.querySelector("[data-recent-toggle] .inbox-recent-count")?.textContent).toBe("1"), WAIT);
    expect(document.querySelector(`[data-key="tracker_task:${taskId}"]`)).toBeNull();
    const { taskWatchIntentAddress } = await import("../src/core/taskReadOrder.js");
    expect((await value(taskWatchIntentAddress(scope, taskId)))?.task_unread_display?.inbox).toBeUndefined();
    await invalidateWithActualIncompleteAction();
    document.querySelector("[data-recent-toggle]").click();
    await vi.waitFor(() => expect(document.querySelector(`[data-key="tracker_task:${taskId}"]`)).not.toBeNull(), WAIT);
    expect(document.querySelector(`[data-key="tracker_task:${taskId}"] .inbox-status-unread`)).toBeNull();
  });
});

// Public producer lifecycle controls: the consumer must have ACTUALLY rendered
// and persisted its value before all mounted owners leave. No display/proof
// state is fabricated. These are separate from the original108 witnesses.
async function displayedNote() {
  const { taskWatchIntentAddress } = await import("../src/core/taskReadOrder.js");
  return value(taskWatchIntentAddress(scope, taskId));
}

async function renderPersistAndLeave() {
  await seedSelectedInputs(); await mountPane();
  await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.["tasks-pane"]?.total).toBe(3), WAIT);
  pane.dispose(); pane = null; paneHost.replaceChildren();
}

async function remountIncompletePane(title) {
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(paneHost, { deviceId: scope.deviceId, projectId: scope.projectId,
    projectKey: project.projectKey, projectName: project.name, defaultView: "list", callRpc: rpc,
    catalog: () => ({ providers: [] }), feed: () => ({ items: [], workspaces: [] }) });
  await vi.waitFor(() => expect(paneHost.querySelector(".task-title")?.textContent).toBe(title), WAIT);
}

async function admittedLineage(watched, watchStartedAfter) {
  const task = full({ title: "Genuine unmounted watch observation", watched,
    watch_started_after: watchStartedAfter, updated_at: "2026-10-29T12:02:00Z" });
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete task[field];
  const address = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord([task], columns(), contentRead.readOrder), { contentRead })).toBe(true);
  const admitted = await listedTask();
  expect(admitted.watched).toBe(watched);
  expect(admitted.__task_watch_cache).toMatchObject({ source: "read", taskId,
    scope, observed_read_order: contentRead.readOrder });
  const { hasTaskWatchSnapshot } = await import("../src/core/taskWatchFacts.js");
  expect(hasTaskWatchSnapshot(admitted)).toBe(true);
  expect(admitted).not.toHaveProperty("unread_count");
  return admitted;
}

async function blindRestorePriorLineage(title) {
  const task = { ...await listedTask(), title, watched: true, watch_started_after: "te-15",
    updated_at: "2026-10-29T12:03:00Z" };
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete task[field];
  expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord([task], columns()))).toBe(true);
  const listed = await listedTask();
  expect(listed).toMatchObject({ watched: true, watch_started_after: "te-15", title });
  expect(listed).not.toHaveProperty("unread_count");
  expect(listed).not.toHaveProperty("__task_watch_cache");
  const { taskUnreadCount } = await import("../src/core/taskUnread.js");
  expect(taskUnreadCount(listed, await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId))).toBe(0);
}

describe("D5 unmounted admitted lineage versus blind paint", () => {
  it.each([[false, null, "unwatch"], [true, "te-90", "new incarnation"]])(
    "does not revive rendered3 after unmounted admitted %s/%s (%s) followed by a blind prior-lineage rewrite",
    async (watched, floor) => {
      await renderPersistAndLeave();
      await admittedLineage(watched, floor);
      await blindRestorePriorLineage("Blind rewrite after admitted transition");
      await remountIncompletePane("Blind rewrite after admitted transition");
      expect(paneHost.querySelector(".task-unread")).toBeNull();
    });

  it("retains rendered3 on remount after a blind-only rewrite without an admitted replacement", async () => {
    await renderPersistAndLeave();
    // Force an actual blind floor change, then restore the previous raw floor.
    const blind = { ...await listedTask(), watch_started_after: "te-90" };
    delete blind.unread_count;
    expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
      tracker.tasksRecord([blind], columns()))).toBe(true);
    await blindRestorePriorLineage("Only blind lineage paint changed");
    await remountIncompletePane("Only blind lineage paint changed");
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
  });

  it("keeps the actual prior display through unmounted optimistic unwatch and exact rejected rollback", async () => {
    await renderPersistAndLeave();
    const watch = await import("../src/core/taskWatchAuthority.js");
    const owner = await watch.beginTaskWatchMutation(scope, taskId, { watched: false });
    expect(owner).not.toBeNull();
    expect((await listedTask()).watched).toBe(false);
    expect((await displayedNote()).task_unread_display["tasks-pane"].total).toBe(3);
    expect(await watch.settleTaskWatchMutation(owner, { rollback: true })).toBe(true);
    expect((await listedTask()).watched).toBe(true);
    expect((await displayedNote()).latest_watch_intent).toMatchObject({ ownerId: owner.id, disposition: "rejected" });
    expect((await displayedNote()).task_unread_display["tasks-pane"].total).toBe(3);
    await remountIncompletePane("Before incomplete action");
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
  });
});

function holdActualDisplayTransactions() {
  const merge = cache.mergeCachedRecordsTogether;
  const waiting = [];
  let blocked = true;
  // Only schedule the actual display transaction later. Its original reducer,
  // addresses, records and return value still run in the real cache unchanged.
  const isDisplayTransaction = (addresses) => addresses.length === 6
    && addresses[0].kind === "tracker-tasks" && addresses[1].kind === "tracker-task"
    && addresses[2].kind === "tracker-task-reads";
  vi.spyOn(cache, "mergeCachedRecordsTogether").mockImplementation((addresses, reducer) => {
    if (!blocked || !isDisplayTransaction(addresses)) return merge(addresses, reducer);
    return new Promise((resolve, reject) => waiting.push(() => merge(addresses, reducer).then(resolve, reject)));
  });
  return {
    get captured() { return waiting.length; },
    release() { blocked = false; return Promise.all(waiting.splice(0).map((resume) => resume())); },
  };
}

async function admitSummaryOnlyUnwatch() {
  const task = full({ title: "Summary-only admitted unwatch", watched: false,
    updated_at: "2026-10-29T12:04:00Z" });
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete task[field];
  const contentRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const ticket = await summaries.beginSummaryQuery(scope, filters, contentRead.readOrder, { contentRead });
  expect(await summaries.landSummaryPage(ticket, { summary: true, tasks: [summaryOf(task)],
    identities: {}, basis: "d5-real-summary-unwatch" }, { read: contentRead.readOrder })).toBe(true);
  const normalized = await value(summaries.summaryAddress(scope, taskId));
  expect(normalized.task).toMatchObject({ watched: false, __task_watch_cache: {
    source: "read", observed_read_order: contentRead.readOrder, taskId, scope } });
  expect(normalized.task).not.toHaveProperty("unread_count");
}

describe("D5 actual delayed display transaction versus admitted summary source", () => {
  it.each([false, true])("does not publish a captured old display after admitted unwatch when an existing display slot is %s", async (hasSlot) => {
    await seedSelectedInputs();
    if (hasSlot) {
      await mountPane();
      await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.["tasks-pane"]?.total).toBe(3), WAIT);
    } else {
      // This is an absent DISPLAY SLOT, not a fabricated assertion that public
      // initial producers never wrote the surrounding task-note record.
      expect((await displayedNote())?.task_unread_display?.["tasks-pane"]).toBeUndefined();
    }
    const held = holdActualDisplayTransactions();
    try {
      if (hasSlot) {
        await publishCoveringTotal(5);
        await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")?.textContent).toBe("5"), WAIT);
      } else await mountPane();
      await vi.waitFor(() => expect(held.captured).toBeGreaterThan(0), WAIT);
      const addresses = [tracker.tasksAddress(scope.deviceId, scope.projectId), tracker.taskAddress(scope.deviceId, scope.projectId, taskId)];
      const before = (await cache.readCachedMany(addresses)).map(cache.recordWriteOf);
      await admitSummaryOnlyUnwatch();
      expect((await cache.readCachedMany(addresses)).map(cache.recordWriteOf)).toEqual(before);
      await held.release();
      expect((await displayedNote())?.task_unread_display?.["tasks-pane"]).toBeUndefined();
    } finally { await held.release(); }
  });
});
