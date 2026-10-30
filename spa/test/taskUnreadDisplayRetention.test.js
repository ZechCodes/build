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

async function mountInboxProducerComponent(expected = 3) {
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
  await vi.waitFor(() => expect(follower.entries()[0]?.unreadCount).toBe(expected), WAIT);
  if (expected > 0) await vi.waitFor(() => expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull(), WAIT);
  else await vi.waitFor(() => expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull(), WAIT);
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

describe("D5 actual consumer choice and mounted retirement", () => {
  it("persists actual cached timeline2 over listed3 with reversed genuine counter-source orders and retains2 on UNKNOWN", async () => {
    await seedSelectedInputs();
    const detailAddress = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
    // Dispatch the detail first, but land its newer ordinary task later. The
    // old renderer selects by the actual cached timeline's ordinary currency,
    // never by comparing the two counters' client dispatch orders.
    const detailRead = await content.captureTaskContentRead(detailAddress, { revalidate: "full" });
    const address = tracker.tasksAddress(scope.deviceId, scope.projectId);
    const listRead = await content.captureTaskContentRead(address, { revalidate: "full" });
    expect(listRead.readOrder).toBeGreaterThan(detailRead.readOrder);
    expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
      tracker.tasksRecord([full()], columns(), listRead.readOrder), { contentRead: listRead })).toBe(true);
    await mountPane();
    await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.["tasks-pane"]?.total).toBe(3), WAIT);
    const timeline = ["tc-16", "tc-17"].map((id) => comment({ id, task_id: taskId,
      author: { kind: "agent", agent_id: "agent" } }));
    expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
      tracker.taskRecord(full({ updated_at: "2026-10-29T12:00:30Z" }), timeline), { contentRead: detailRead })).toBe(true);
    const listed = await listedTask(), detailed = await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
    const totalSource = (task) => task.__task_family_cache?.total || task.__task_family_cache?.paint?.total;
    expect(totalSource(listed).observed_read_order).toBe(listRead.readOrder);
    expect(totalSource(detailed.task).observed_read_order).toBe(detailRead.readOrder);
    const { taskUnreadCount } = await import("../src/core/taskUnread.js");
    expect(taskUnreadCount(listed, detailed)).toBe(2);
    await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")?.textContent).toBe("2"), WAIT);
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(paneHost.querySelector(".task-title")?.textContent).toBe("After incomplete action"), WAIT);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("2");
    await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.["tasks-pane"]?.total).toBe(2), WAIT);
  });

  it("does not recreate an old display on a fresh actual Pane paint after an admitted summary-only unwatch announcement", async () => {
    await seedSelectedInputs(); await mountPane();
    await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.["tasks-pane"]?.total).toBe(3), WAIT);
    await admitSummaryOnlyUnwatch();
    // A genuine admitted unwatch retires the display even though its summary
    // publication intentionally leaves legacy list/detail bytes unchanged.
    await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")).toBeNull(), WAIT);
    pane.feedMoved();
    expect(paneHost.querySelector(".task-unread")).toBeNull();
    expect((await displayedNote())?.task_unread_display?.["tasks-pane"]).toBeUndefined();
    expect((await listedTask()).unread_count).toBe(3);
  });

  it("retires quota-only mounted display memory on a genuine admitted summary unwatch without requiring a prior persisted slot", async () => {
    await seedSelectedInputs();
    let refused = 0;
    const put = IDBObjectStore.prototype.put;
    vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
      if (record?.value?.task_unread_display?.["tasks-pane"]) {
        refused += 1; throw new DOMException("Mounted display quota refused", "QuotaExceededError");
      }
      return put.call(this, record, key);
    });
    await mountPane();
    await vi.waitFor(() => expect(refused).toBeGreaterThan(0), WAIT);
    expect((await displayedNote())?.task_unread_display?.["tasks-pane"]).toBeUndefined();
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    await admitSummaryOnlyUnwatch();
    await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")).toBeNull(), WAIT);
    expect((await displayedNote())?.task_unread_display?.["tasks-pane"]).toBeUndefined();
    expect(paneHost.querySelector(".error, .warn:not([hidden])")).toBeNull();
  });
});

describe("D5 both actual display owners and the mounted Inbox shell", () => {
  it("keeps independent actual Pane3 and Inbox2 slots through UNKNOWN then retires both on admission while preserving real intent and highwater note fields", async () => {
    await seedSelectedInputs();
    const watch = await import("../src/core/taskWatchAuthority.js");
    const owner = await watch.beginTaskWatchMutation(scope, taskId, { watched: true, metadataOnly: true });
    expect(owner).not.toBeNull();
    expect(await watch.settleTaskWatchMutation(owner, { rollback: true })).toBe(true);
    await mountPane();
    await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.["tasks-pane"]?.total).toBe(3), WAIT);
    pane.dispose(); pane = null; paneHost.replaceChildren();
    const address = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
    const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
    const timeline = ["tc-16", "tc-17"].map((id) => comment({ id, task_id: taskId,
      author: { kind: "agent", agent_id: "agent" } }));
    expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
      tracker.taskRecord(full({ updated_at: "2026-10-29T12:00:30Z" }), timeline), { contentRead })).toBe(true);
    await mountInboxProducerComponent(2);
    await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.inbox?.total).toBe(2), WAIT);
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(inboxHost.querySelector(".stitle")?.textContent).toContain("After incomplete action"), WAIT);
    expect(follower.entries()[0].unreadCount).toBe(2);
    await remountIncompletePane("After incomplete action");
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    const before = await displayedNote();
    expect(before.task_unread_display["tasks-pane"].total).toBe(3);
    expect(before.task_unread_display.inbox.total).toBe(2);
    expect(before.latest_watch_intent).toMatchObject({ ownerId: owner.id, disposition: "rejected" });
    const { task_unread_display: _beforeDisplay, ...otherBefore } = before;
    await admitSummaryOnlyUnwatch();
    const after = await displayedNote();
    const { task_unread_display: _afterDisplay, ...otherAfter } = after;
    expect(otherAfter).toEqual(otherBefore);
    expect(after.task_unread_display?.["tasks-pane"]).toBeUndefined();
    expect(after.task_unread_display?.inbox).toBeUndefined();
    await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")).toBeNull(), WAIT);
    await vi.waitFor(() => expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull(), WAIT);
  });

  it("retains actual full Inbox shell display3 on UNKNOWN then removes its indicator after genuine summary-only unwatch", async () => {
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
    document.querySelector("[data-recent-toggle]").click();
    const indicator = () => document.querySelector(`[data-key="tracker_task:${taskId}"] .inbox-status-unread`);
    await vi.waitFor(() => expect(indicator()).not.toBeNull(), WAIT);
    await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.inbox?.total).toBe(3), WAIT);
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(document.querySelector(`[data-key="tracker_task:${taskId}"] .stitle`)?.textContent).toContain("After incomplete action"), WAIT);
    expect(indicator()).not.toBeNull();
    expect((await displayedNote()).task_unread_display.inbox.total).toBe(3);
    await admitSummaryOnlyUnwatch();
    await vi.waitFor(() => expect(indicator()).toBeNull(), WAIT);
    expect((await displayedNote())?.task_unread_display?.inbox).toBeUndefined();
  });
});

describe("D5 actual producer transaction and non-admission controls", () => {
  it("rolls back an attempted unwatch and both actual display slots when the summary transaction aborts after a companion was queued", async () => {
    await seedSelectedInputs(); await mountPane(); await mountInboxProducerComponent();
    await vi.waitFor(async () => {
      const note = await displayedNote();
      expect(note?.task_unread_display?.["tasks-pane"]?.total).toBe(3);
      expect(note?.task_unread_display?.inbox?.total).toBe(3);
    }, WAIT);
    const contentRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    const ticket = await summaries.beginSummaryQuery(scope, filters, contentRead.readOrder, { contentRead });
    const { taskWatchIntentAddress } = await import("../src/core/taskReadOrder.js");
    const { recordKey } = await import("../src/core/idbRecords.js");
    const rowAddress = summaries.summaryAddress(scope, taskId);
    const addresses = [ticket.address, rowAddress, taskWatchIntentAddress(scope, taskId)];
    const before = await cache.readCachedMany(addresses);
    const queryKey = recordKey(ticket.address), rowKey = recordKey(rowAddress);
    let queuedCompanion = false, refused = 0;
    const put = IDBObjectStore.prototype.put;
    vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
      if (key === queryKey) queuedCompanion = true;
      if (key === rowKey && record?.value?.task?.watched === false) {
        expect(queuedCompanion).toBe(true);
        refused += 1; throw new DOMException("Atomic watch publication refused", "QuotaExceededError");
      }
      return put.call(this, record, key);
    });
    const task = full({ title: "Must abort with its display retirement", watched: false,
      updated_at: "2026-10-29T12:04:00Z" });
    expect(await summaries.landSummaryPage(ticket, { summary: true, tasks: [summaryOf(task)],
      identities: {}, basis: "d5-companion-abort" }, { read: contentRead.readOrder })).toBe(false);
    expect(queuedCompanion).toBe(true); expect(refused).toBe(1);
    expect(await cache.readCachedMany(addresses)).toEqual(before);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull();
    expect(follower.entries()[0].unreadCount).toBe(3);
  });

  it("keeps both actual display slots when an old summary unwatch is refused by a newer query reservation", async () => {
    await seedSelectedInputs(); await mountPane(); await mountInboxProducerComponent();
    await vi.waitFor(async () => {
      const note = await displayedNote();
      expect(note?.task_unread_display?.["tasks-pane"]?.total).toBe(3);
      expect(note?.task_unread_display?.inbox?.total).toBe(3);
    }, WAIT);
    const older = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    const ticket = await summaries.beginSummaryQuery(scope, filters, older.readOrder, { contentRead: older });
    const newer = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    await summaries.beginSummaryQuery(scope, filters, newer.readOrder, { contentRead: newer });
    const before = await displayedNote();
    const task = full({ title: "Refused stale unwatch", watched: false, updated_at: "2026-10-29T12:04:00Z" });
    expect(await summaries.landSummaryPage(ticket, { summary: true, tasks: [summaryOf(task)],
      identities: {}, basis: "must-not-retire" }, { read: older.readOrder })).toBe(false);
    expect(await displayedNote()).toEqual(before);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    expect(follower.entries()[0].unreadCount).toBe(3);
  });
});

async function renderInboxPersistAndLeave() {
  await seedSelectedInputs(); await mountInboxProducerComponent();
  await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.inbox?.total).toBe(3), WAIT);
  follower.dispose(); follower = null; inboxHost.replaceChildren();
}

describe("D5 Inbox production render owner lifecycle counterparts", () => {
  it.each([[false, null, "unwatch"], [true, "te-90", "new incarnation"]])(
    "does not revive Inbox3 after unmounted admitted %s/%s (%s) and a blind prior-lineage rewrite",
    async (watched, floor) => {
      await renderInboxPersistAndLeave();
      await admittedLineage(watched, floor);
      await blindRestorePriorLineage("Inbox blind rewrite after admission");
      await mountInboxProducerComponent(0);
      expect(inboxHost.querySelector(".stitle")?.textContent).toContain("Inbox blind rewrite after admission");
      expect(follower.entries()[0].unreadCount).toBe(0);
      expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull();
    });

  it("retains actual Inbox3 on remount after only blind lineage changes", async () => {
    await renderInboxPersistAndLeave();
    const blind = { ...await listedTask(), watch_started_after: "te-90" };
    delete blind.unread_count;
    expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
      tracker.tasksRecord([blind], columns()))).toBe(true);
    await blindRestorePriorLineage("Inbox blind-only paint");
    await mountInboxProducerComponent();
    expect(inboxHost.querySelector(".stitle")?.textContent).toContain("Inbox blind-only paint");
    expect(follower.entries()[0].unreadCount).toBe(3);
  });

  it("keeps actual Inbox3 through unmounted optimistic unwatch and exact rejected rollback", async () => {
    await renderInboxPersistAndLeave();
    const watch = await import("../src/core/taskWatchAuthority.js");
    const owner = await watch.beginTaskWatchMutation(scope, taskId, { watched: false });
    expect(owner).not.toBeNull();
    expect((await listedTask()).watched).toBe(false);
    expect((await displayedNote()).task_unread_display.inbox.total).toBe(3);
    expect(await watch.settleTaskWatchMutation(owner, { rollback: true })).toBe(true);
    expect((await displayedNote()).latest_watch_intent).toMatchObject({ ownerId: owner.id, disposition: "rejected" });
    expect((await displayedNote()).task_unread_display.inbox.total).toBe(3);
    await mountInboxProducerComponent();
    expect(follower.entries()[0].unreadCount).toBe(3);
  });

  it.each([false, true])("does not publish a captured Inbox display after admitted unwatch when an existing Inbox slot is %s", async (hasSlot) => {
    await seedSelectedInputs();
    if (hasSlot) {
      await mountInboxProducerComponent();
      await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.inbox?.total).toBe(3), WAIT);
    } else expect((await displayedNote())?.task_unread_display?.inbox).toBeUndefined();
    const held = holdActualDisplayTransactions();
    try {
      if (hasSlot) {
        await publishCoveringTotal(5);
        await vi.waitFor(() => expect(follower.entries()[0]?.unreadCount).toBe(5), WAIT);
        expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull();
      } else await mountInboxProducerComponent();
      await vi.waitFor(() => expect(held.captured).toBeGreaterThan(0), WAIT);
      const addresses = [tracker.tasksAddress(scope.deviceId, scope.projectId), tracker.taskAddress(scope.deviceId, scope.projectId, taskId)];
      const before = (await cache.readCachedMany(addresses)).map(cache.recordWriteOf);
      await admitSummaryOnlyUnwatch();
      expect((await cache.readCachedMany(addresses)).map(cache.recordWriteOf)).toEqual(before);
      await held.release();
      expect((await displayedNote())?.task_unread_display?.inbox).toBeUndefined();
    } finally { await held.release(); }
  });

  it("does not recreate actual Inbox display on a fresh renderEntry paint after an admitted unwatch announcement", async () => {
    await seedSelectedInputs(); await mountInboxProducerComponent();
    await vi.waitFor(async () => expect((await displayedNote())?.task_unread_display?.inbox?.total).toBe(3), WAIT);
    await admitSummaryOnlyUnwatch();
    await vi.waitFor(() => expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull(), WAIT);
    const { inboxRowHtml } = await import("../src/core/inbox.js");
    inboxHost.innerHTML = follower.entries().map((entry) => follower.renderEntry(entry, () => inboxRowHtml(entry))).join("");
    expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull();
    expect(follower.entries()[0].unreadCount).toBe(0);
    expect((await displayedNote())?.task_unread_display?.inbox).toBeUndefined();
  });

  it("retires actual Inbox quota-only display memory on admitted unwatch without a persisted Inbox slot", async () => {
    await seedSelectedInputs();
    let refused = 0;
    const put = IDBObjectStore.prototype.put;
    vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
      if (record?.value?.task_unread_display?.inbox) {
        refused += 1; throw new DOMException("Inbox display quota refused", "QuotaExceededError");
      }
      return put.call(this, record, key);
    });
    await mountInboxProducerComponent();
    await vi.waitFor(() => expect(refused).toBeGreaterThan(0), WAIT);
    expect((await displayedNote())?.task_unread_display?.inbox).toBeUndefined();
    expect(follower.entries()[0].unreadCount).toBe(3);
    await admitSummaryOnlyUnwatch();
    await vi.waitFor(() => expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull(), WAIT);
    expect(follower.entries()[0].unreadCount).toBe(0);
    expect((await displayedNote())?.task_unread_display?.inbox).toBeUndefined();
    expect(inboxHost.querySelector(".error, .warn:not([hidden])")).toBeNull();
  });
});

async function bothActualSlots() {
  await seedSelectedInputs(); await mountPane(); await mountInboxProducerComponent();
  await vi.waitFor(async () => {
    const note = await displayedNote();
    expect(note?.task_unread_display?.["tasks-pane"]?.total).toBe(3);
    expect(note?.task_unread_display?.inbox?.total).toBe(3);
  }, WAIT);
}

async function noteRecord() {
  const { taskWatchIntentAddress } = await import("../src/core/taskReadOrder.js");
  return cache.readCached(taskWatchIntentAddress(scope, taskId));
}

async function observedSummaryMutation(contentRead, extra = {}) {
  const task = summaryOf(full({ title: "Observed same watch incarnation", updated_at: "2026-10-29T12:04:00Z", ...extra }));
  expect(await summaries.writeSummaryTask(scope, task, contentRead.readOrder, { contentRead })).toBe(true);
  return (await value(summaries.summaryAddress(scope, taskId))).task;
}

describe("D5 real producer ordering, copies, partial updates and recovery", () => {
  it("advances the same-task publication note on a genuinely newer same-lineage admission without retiring either actual display slot", async () => {
    await bothActualSlots();
    const before = await noteRecord();
    const read = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    const observed = await observedSummaryMutation(read);
    expect(observed).toMatchObject({ watched: true, watch_started_after: "te-15",
      __task_watch_cache: { source: "read", observed_read_order: read.readOrder, taskId, scope } });
    const after = await noteRecord();
    expect(after.value.task_unread_display["tasks-pane"]).toEqual(before.value.task_unread_display["tasks-pane"]);
    expect(after.value.task_unread_display.inbox).toEqual(before.value.task_unread_display.inbox);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    expect(follower.entries()[0].unreadCount).toBe(3);
    // Equality tokens only prove a committed note update; they do not order
    // sources or certify a count. The genuine selected observation is above.
    expect(cache.recordWriteOf(after)).not.toBe(cache.recordWriteOf(before));
  });

  it("keeps the first actual normalized source on identical and conflicting equal-read replays without mutating either display slot", async () => {
    await bothActualSlots();
    const read = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    await observedSummaryMutation(read);
    const before = await noteRecord();
    const identical = await observedSummaryMutation(read);
    expect(identical.watched).toBe(true);
    expect(await noteRecord()).toEqual(before);
    const conflict = await observedSummaryMutation(read, { watched: false });
    expect(conflict).toMatchObject({ watched: true, watch_started_after: "te-15",
      __task_watch_cache: { observed_read_order: read.readOrder } });
    expect(await noteRecord()).toEqual(before);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    expect(follower.entries()[0].unreadCount).toBe(3);
    // The actual source selector retains the first source here, so these
    // replays are not evidence that a second equal conflict was admitted.
  });

  it("does not turn a copied observed row or a compact read-only patch into a display retirement event", async () => {
    await bothActualSlots();
    const order = await import("../src/core/taskReadOrder.js");
    const projected = (await value(summaries.summaryAddress(scope, taskId))).task;
    const before = await noteRecord();
    expect(await summaries.writeSummaryTask(scope, { ...projected, title: "Copied ordinary row" }, await order.nextTaskRead())).toBe(true);
    expect(await noteRecord()).toEqual(before);
    expect(await summaries.writeSummaryTask(scope, { id: taskId, read_through: "tc-20" }, await order.nextTaskRead())).toBe(true);
    const normalized = await value(summaries.summaryAddress(scope, taskId));
    expect(normalized.needs_unread_refresh).toBe(true);
    expect(normalized.task.read_through).toBe("tc-20");
    expect(normalized.task).not.toHaveProperty("unread_count");
    expect(normalized.task.__task_family_cache?.unknown?.total).toBeDefined();
    expect(await noteRecord()).toEqual(before);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    expect(follower.entries()[0].unreadCount).toBe(3);
  });

  it("replaces both actually retained displays with genuine later covering zero and value", async () => {
    await bothActualSlots();
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(paneHost.querySelector(".task-title")?.textContent).toBe("After incomplete action"), WAIT);
    await vi.waitFor(() => expect(inboxHost.querySelector(".stitle")?.textContent).toContain("After incomplete action"), WAIT);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    expect(follower.entries()[0].unreadCount).toBe(3);
    for (const total of [0, 5]) {
      await publishCoveringTotal(total);
      await vi.waitFor(() => expect(follower.entries()[0]?.unreadCount).toBe(total), WAIT);
      await vi.waitFor(async () => {
        const note = await displayedNote();
        expect(note?.task_unread_display?.["tasks-pane"]?.total).toBe(total);
        expect(note?.task_unread_display?.inbox?.total).toBe(total);
      }, WAIT);
      if (total === 0) {
        expect(paneHost.querySelector(".task-unread")).toBeNull();
        expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull();
      } else {
        expect(paneHost.querySelector(".task-unread")?.textContent).toBe("5");
        expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull();
      }
    }
  });

  it("does not let an old-lifetime summary reply retire newly rendered slots after real project eviction and re-admission", async () => {
    await bothActualSlots();
    const older = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    const ticket = await summaries.beginSummaryQuery(scope, filters, older.readOrder, { contentRead: older });
    pane.dispose(); pane = null; follower.dispose(); follower = null;
    paneHost.replaceChildren(); inboxHost.replaceChildren();
    await cache.evictEntity(scope.deviceId, scope.projectId);
    await bothActualSlots();
    const current = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    expect(current.ticket.scopeGeneration).not.toBe(older.ticket.scopeGeneration);
    const before = await noteRecord();
    const task = full({ title: "Old lifetime cannot unwatch this display", watched: false });
    expect(await summaries.landSummaryPage(ticket, { summary: true, tasks: [summaryOf(task)],
      identities: {}, basis: "old-lifetime" }, { read: older.readOrder })).toBe(false);
    expect(await noteRecord()).toEqual(before);
    expect(paneHost.querySelector(".task-unread")?.textContent).toBe("3");
    expect(follower.entries()[0].unreadCount).toBe(3);
  });
});

// Coordinator4641 conditional branch: exhaustive production routing audit is
// clean (task891-terra61-inbox-routing-audit.md). Preserve the ORIGINAL unhooked
// test and its raw1. This separately named correction uses the production
// patchList/renderEntry callback; it does not seed display during enumeration.
async function mountInboxProductionPatchComponent() {
  for (const [kind, held] of [["projects", [project]], ["workspaces", []],
    ["feed", { projects: [project], workspaces: [], runs: [], items: [] }]]) {
    await cache.writeCached({ deviceId: scope.deviceId, entityId: "", kind }, held);
  }
  const { followWatchedTasks } = await import("../src/core/watchedTaskFollower.js");
  const { inboxRowHtml } = await import("../src/core/inbox.js");
  const { patchList } = await import("../src/core/patchList.js");
  follower = followWatchedTasks({ onChange: () => patchList(inboxHost, follower.entries(), {
    keyOf: (entry) => entry.key,
    render: (entry) => follower.renderEntry(entry, () => inboxRowHtml(entry)),
  }) });
  follower.follow([project]);
  await vi.waitFor(() => expect(follower.entries()[0]?.unreadCount).toBe(3), WAIT);
  await vi.waitFor(() => expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull(), WAIT);
}

describe("D5 coordinator4641 corrected production fixture", () => {
  it("corrected production patchList renderEntry fixture retains the followed Inbox row's previously displayed 3 when an incomplete full action makes total authority UNKNOWN", async () => {
    await seedSelectedInputs();
    await mountInboxProductionPatchComponent();
    await invalidateWithActualIncompleteAction();
    await vi.waitFor(() => expect(inboxHost.querySelector(".stitle")?.textContent).toContain("After incomplete action"), WAIT);
    expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull();
    expect(follower.entries()[0].unreadCount).toBe(3);
  });
});

async function admitSameFloorSummaryWithoutCounts(title, updatedAt) {
  const task = full({ title, read_through: "tc-10", watched: true, watch_started_after: "te-15", updated_at: updatedAt });
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete task[field];
  const read = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const ticket = await summaries.beginSummaryQuery(scope, filters, read.readOrder, { contentRead: read });
  expect(await summaries.landSummaryPage(ticket, { summary: true, tasks: [summaryOf(task)],
    identities: {}, basis: title }, { read: read.readOrder })).toBe(true);
  const normalized = await value(summaries.summaryAddress(scope, taskId));
  expect(normalized.task).toMatchObject({ watched: true, watch_started_after: "te-15",
    __task_watch_cache: { source: "read", observed_read_order: read.readOrder, taskId, scope } });
  expect(normalized.needs_unread_refresh).toBe(true);
  expect(normalized.task).not.toHaveProperty("unread_count");
  return read.readOrder;
}

it("does not revive old same-tuple display across an admitted unwatch and rewatch then preserves a genuinely covered value across later same-lineage admission", async () => {
  await bothActualSlots();
  const oldSource = (await listedTask()).__task_watch_cache.observed_read_order;
  const addresses = [tracker.tasksAddress(scope.deviceId, scope.projectId), tracker.taskAddress(scope.deviceId, scope.projectId, taskId)];
  const oldTokens = (await cache.readCachedMany(addresses)).map(cache.recordWriteOf);
  await admitSummaryOnlyUnwatch();
  await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")).toBeNull(), WAIT);
  await vi.waitFor(() => expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull(), WAIT);
  const boundary = await admitSameFloorSummaryWithoutCounts("Rewatch with the SAME previous tuple", "2026-10-29T12:05:00Z");
  expect(boundary).toBeGreaterThan(oldSource);
  expect((await cache.readCachedMany(addresses)).map(cache.recordWriteOf)).toEqual(oldTokens);
  expect((await listedTask()).unread_count).toBe(3);
  expect((await listedTask()).watch_started_after).toBe("te-15");
  expect(paneHost.querySelector(".task-unread")).toBeNull();
  expect(follower.entries()[0].unreadCount).toBe(0);
  expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull();
  await publishCoveringTotal(5);
  await vi.waitFor(() => expect(paneHost.querySelector(".task-unread")?.textContent).toBe("5"), WAIT);
  await vi.waitFor(() => expect(follower.entries()[0]?.unreadCount).toBe(5), WAIT);
  const coveredSource = (await listedTask()).__task_watch_cache.observed_read_order;
  expect(coveredSource).toBeGreaterThan(boundary);
  const newerSameLineage = await admitSameFloorSummaryWithoutCounts("Later same-lineage observation", "2026-10-29T12:06:00Z");
  expect(newerSameLineage).toBeGreaterThan(coveredSource);
  pane.feedMoved();
  expect(paneHost.querySelector(".task-unread")?.textContent).toBe("5");
  expect(follower.entries()[0].unreadCount).toBe(5);
  expect(inboxHost.querySelector(".inbox-status-unread")).not.toBeNull();
  await vi.waitFor(async () => {
    const note = await displayedNote();
    expect(note?.task_unread_display?.["tasks-pane"]?.total).toBe(5);
    expect(note?.task_unread_display?.inbox?.total).toBe(5);
  }, WAIT);
});

function leaveBothActualConsumers() {
  pane.dispose(); pane = null; follower.dispose(); follower = null;
  paneHost.replaceChildren(); inboxHost.replaceChildren();
}

const selectedWatchIdentity = (task) => {
  const { lineage: _lineage, uncertain: _uncertain, ...identity } = task.__task_watch_cache;
  return identity;
};

describe("D5 actual admitted event identity and selected value origin", () => {
  it("keeps the first shared-note event and diagnoses conflicting lineages independently admitted from the same real capture by summary and catalogue producers", async () => {
    await bothActualSlots(); leaveBothActualConsumers();
    const read = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    const normalized = await observedSummaryMutation(read);
    expect(normalized).toMatchObject({ watched: true, watch_started_after: "te-15",
      __task_watch_cache: { source: "read", observed_read_order: read.readOrder, taskId, scope } });
    const first = await noteRecord();
    await observedSummaryMutation(read);
    expect(await noteRecord()).toEqual(first);

    const diagnostic = vi.spyOn(console, "info").mockImplementation(() => {});
    const conflicting = full({ watched: false, title: "Independent catalogue admission at the SAME source",
      updated_at: "2026-10-29T12:05:00Z" });
    expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
      tracker.tasksRecord([conflicting], columns(), read.readOrder), { contentRead: read })).toBe(true);
    const catalogue = await listedTask();
    // Existing-ID family admission does not claim list membership. Both real
    // selectors independently selected this genuine capture, unlike the
    // earlier normalized equal-read replay that retained its first lineage.
    expect(catalogue).toMatchObject({ watched: false,
      __task_watch_cache: { source: "read", observed_read_order: read.readOrder, taskId, scope } });
    expect(selectedWatchIdentity(catalogue)).toEqual(selectedWatchIdentity(normalized));
    expect((await value(summaries.summaryAddress(scope, taskId))).task.watched).toBe(true);
    expect(await noteRecord()).toEqual(first);
    expect(diagnostic).toHaveBeenCalledTimes(1);
  });

  it("does not associate an old listed total with the new watch source when the Pane remounts after a same-tuple watch cycle", async () => {
    await retainOldTotalUnderNewWatch();
    await remountIncompletePane("New watch source with retained OLD total");
    expect(paneHost.querySelector(".task-unread")).toBeNull();
    expect((await displayedNote())?.task_unread_display?.["tasks-pane"]).toBeUndefined();
  });

  it("does not associate an old listed total with the new watch source when the Inbox remounts after a same-tuple watch cycle", async () => {
    await retainOldTotalUnderNewWatch();
    await mountInboxProducerComponent(0);
    expect(inboxHost.querySelector(".inbox-status-unread")).toBeNull();
    expect((await displayedNote())?.task_unread_display?.inbox).toBeUndefined();
  });
});

async function retainOldTotalUnderNewWatch() {
  await bothActualSlots(); leaveBothActualConsumers();
  const original = await listedTask();
  const oldTotal = original.__task_family_cache.total || original.__task_family_cache.paint?.total;
  expect(oldTotal).toMatchObject({ observed_read_order: original.__task_watch_cache.observed_read_order });
  await admitSummaryOnlyUnwatch();
  await admitSameFloorSummaryWithoutCounts("Rewatch before selecting a value", "2026-10-29T12:05:00Z");
  const task = full({ title: "New watch source with retained OLD total", updated_at: "2026-10-29T12:06:00Z" });
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete task[field];
  const address = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const read = await content.captureTaskContentRead(address, { revalidate: "full" });
  expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord([task], columns(), read.readOrder), { contentRead: read })).toBe(true);
  const selected = await listedTask();
  const selectedTotal = selected.__task_family_cache.total || selected.__task_family_cache.paint?.total;
  expect(selected).toMatchObject({ watched: true, watch_started_after: "te-15", unread_count: 3,
    __task_watch_cache: { observed_read_order: read.readOrder, source: "read", taskId, scope } });
  expect(selectedTotal).toEqual(oldTotal);
  expect(selectedTotal.observed_read_order).toBeLessThan(read.readOrder);
  const normalized = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(normalized).toMatchObject({ watched: true, watch_started_after: "te-15" });
  expect(normalized).not.toHaveProperty("unread_count");
}
