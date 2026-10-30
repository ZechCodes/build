// @vitest-environment jsdom
// Test-first consumer boundaries through CURRENT public cache/render APIs.
// No projection lease/export scaffold, private display seed or mocked reducer.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { columns, task as wireTask } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] }, tasks: {} }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const scope = { deviceId: "d5-consumer-device", projectId: "proj-1" };
const taskId = "d5-consumer-task";
const project = { id: scope.projectId, project_id: scope.projectId, deviceId: scope.deviceId,
  projectKey: `${scope.deviceId}|${scope.projectId}`, name: "Build" };
const WAIT = { timeout: 3000, interval: 10 };
const full = (extra = {}) => wireTask({ id: taskId, project_id: scope.projectId, number: 18,
  title: "Actually selected consumer3", status: "ready", assignee: { kind: "user" }, watched: true,
  read_through: "tc-10", watch_started_after: "te-15", dismissed_through: "te-05",
  unread_count: 3, unread_mentions: 1, unread_agent_comments: 2,
  updated_at: "2026-10-30T12:00:00Z", attachments: [], ...extra });
const inboxRow = () => document.querySelector(`[data-key="tracker_task:${taskId}"]`);

let cache, tracker, content, pane, shell, taskFeed, follower, delayed, sourceReads;
const isDisplayAddresses = (addresses) => addresses.length === 6
  && addresses[0].kind === "tracker-tasks" && addresses[1].kind === "tracker-task"
  && addresses[2].kind === "tracker-task-reads";

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-10-30T12:00:30Z"));
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  sessionStorage.clear();
  document.body.innerHTML = '<div id="consumer-pane"></div><div id="consumer-row"></div>';
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  content = await import("../src/core/taskContentAuthority.js");
  sourceReads = new Set();
  const read = cache.readCachedMany;
  vi.spyOn(cache, "readCachedMany").mockImplementation((...args) => {
    const actual = read(...args);
    if (isDisplayAddresses(args[0])) {
      sourceReads.add(actual);
      void actual.then(() => sourceReads.delete(actual), () => sourceReads.delete(actual));
    }
    return actual;
  });
  pane = null; shell = null; taskFeed = null; follower = null; delayed = null;
});

afterEach(async () => {
  await delayed?.release();
  pane?.dispose();
  follower?.dispose();
  shell?.unmountInboxList();
  taskFeed?.stopFeed();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

async function note() {
  const { taskWatchIntentAddress } = await import("../src/core/taskReadOrder.js");
  return (await cache.readCached(taskWatchIntentAddress(scope, taskId)))?.value;
}

async function sourceReadbacks() {
  // Await the ORIGINAL actually invoked display-source reads, without changing
  // their result or ordering. This is not a sleep or an inferred receipt clock.
  await Promise.all([...sourceReads]);
  await Promise.resolve();
}

async function seedActualInputs() {
  const detailAddress = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
  const detailRead = await content.captureTaskContentRead(detailAddress, { revalidate: "full" });
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
    tracker.taskRecord(full({ title: "Older genuine detail", updated_at: "2026-10-30T11:59:00Z" }), []),
    { contentRead: detailRead })).toBe(true);
  await publishActualList(3);
  const listed = (await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0];
  const detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  const { taskUnreadCount } = await import("../src/core/taskUnread.js");
  expect(listed.unread_count).toBe(3);
  expect(detail.timeline).toEqual([]);
  expect(taskUnreadCount(listed, detail)).toBe(3);
  expect((await note())?.task_unread_display).toBeUndefined();
}

async function publishActualList(total) {
  const address = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const read = await content.captureTaskContentRead(address, { revalidate: "full" });
  const task = full({ unread_count: total, updated_at: total === 3 ? "2026-10-30T12:00:00Z" : "2026-10-30T12:00:10Z" });
  expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord([task], columns(), read.readOrder), { contentRead: read })).toBe(true);
}

function delayActualDisplayMerges() {
  const merge = cache.mergeCachedRecordsTogether;
  const waiting = [];
  let blocked = true;
  vi.spyOn(cache, "mergeCachedRecordsTogether").mockImplementation((...args) => {
    if (!blocked || !isDisplayAddresses(args[0])) return merge(...args);
    // Schedule the ORIGINAL real merge/addresses/reducer/options later. No
    // record, selected count, native transaction result or guard is invented.
    return new Promise((resolve, reject) => waiting.push(() => merge(...args).then(resolve, reject)));
  });
  delayed = {
    get captured() { return waiting.length; },
    async release() {
      blocked = false;
      await Promise.all(waiting.splice(0).map((resume) => resume()));
    },
  };
  return delayed;
}

async function mountActualPane() {
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(document.querySelector("#consumer-pane"), { ...scope,
    projectKey: project.projectKey, projectName: project.name, defaultView: "list",
    callRpc: () => new Promise(() => {}), catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }) });
  await vi.waitFor(() => expect(document.querySelector("#consumer-pane .task-unread")?.textContent).toBe("3"), WAIT);
}

async function mountActualInbox(view = "inbox") {
  document.body.innerHTML = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
  const { App } = await import("../src/app.js");
  Object.assign(App, { route: { name: "inbox" }, devices: [{ id: scope.deviceId, name: "Consumer device", status: "online" }],
    selectedDeviceId: scope.deviceId, deviceFilter: null });
  for (const [kind, value] of [["projects", [project]], ["workspaces", []],
    ["feed", { projects: [project], workspaces: [], runs: [], items: [] }]]) {
    await cache.writeCached({ deviceId: scope.deviceId, entityId: "", kind }, value);
  }
  shell = await import("../src/core/inboxView.js");
  taskFeed = await import("../src/core/taskFeed.js");
  shell.setInboxView(view); shell.mountInboxList(); await taskFeed.startFeed();
  await vi.waitFor(() => {
    expect(inboxRow()).not.toBeNull();
    expect(inboxRow().querySelector(".inbox-status-unread")).not.toBeNull();
  }, WAIT);
  expect(inboxRow()?.querySelector(".stitle")?.textContent).toContain("Actually selected consumer3");
}

function failActualTemplateAfterHtml(matches, beforeFailure = () => {}) {
  const descriptor = Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML");
  const failure = new Error("Deliberate consumer DOM failure AFTER real row HTML");
  let reached = 0;
  const setter = vi.spyOn(Element.prototype, "innerHTML", "set").mockImplementation(function (html) {
    descriptor.set.call(this, html);
    if (reached || this.tagName !== "TEMPLATE" || !matches(String(html))) return;
    reached += 1;
    beforeFailure();
    throw failure;
  });
  return { failure, get reached() { return reached; }, restore: () => setter.mockRestore() };
}

describe("D5 first actual consumer lifecycle boundaries", () => {
  it("preserves the prior visible Pane selection without creating persistence from a later failed actual DOM paint", async () => {
    await seedActualInputs(); const writes = delayActualDisplayMerges();
    await mountActualPane();
    await vi.waitFor(() => expect(writes.captured).toBeGreaterThan(0), WAIT);
    await sourceReadbacks(); const before = writes.captured;
    const failed = failActualTemplateAfterHtml((html) => html.includes(`data-task="${taskId}"`));
    try { expect(() => pane.feedMoved()).toThrow(failed.failure); } finally { failed.restore(); }
    expect(failed.reached).toBe(1);
    await sourceReadbacks();
    expect(writes.captured).toBe(before);
    await writes.release();
    expect((await note())?.task_unread_display?.["tasks-pane"]?.total).toBe(3);
  });

  it("does not create Inbox persistence from the real HTML callback before a failed full-shell DOM paint succeeds", async () => {
    await seedActualInputs(); const writes = delayActualDisplayMerges();
    await mountActualInbox();
    await vi.waitFor(() => expect(writes.captured).toBeGreaterThan(0), WAIT);
    await sourceReadbacks(); const before = writes.captured;
    const failed = failActualTemplateAfterHtml((html) => html.includes(`tracker_task:${taskId}`));
    try { expect(() => shell.inboxListRouteChanged()).toThrow(failed.failure); } finally { failed.restore(); }
    expect(failed.reached).toBe(1);
    await sourceReadbacks();
    expect(writes.captured).toBe(before);
    await writes.release();
    expect((await note())?.task_unread_display?.inbox?.total).toBe(3);
  });

  it("publishes only the successful inner full-Inbox draw when a reentrant outer HTML paint later fails", async () => {
    await seedActualInputs(); const writes = delayActualDisplayMerges();
    await mountActualInbox();
    await vi.waitFor(() => expect(writes.captured).toBeGreaterThan(0), WAIT);
    await sourceReadbacks(); const before = writes.captured;
    const failed = failActualTemplateAfterHtml((html) => html.includes(`tracker_task:${taskId}`),
      () => shell.inboxListRouteChanged());
    try { expect(() => shell.inboxListRouteChanged()).toThrow(failed.failure); } finally { failed.restore(); }
    expect(failed.reached).toBe(1);
    expect(inboxRow()).not.toBeNull();
    expect(inboxRow().querySelector(".inbox-status-unread")).not.toBeNull();
    await sourceReadbacks();
    expect(writes.captured).toBe(before + 1);
    await writes.release();
    expect((await note())?.task_unread_display?.inbox?.total).toBe(3);
  });

  it("does not seed UNKNOWN Inbox history by rebinding a stale cloned actual entry to a later task source", async () => {
    await seedActualInputs();
    const { followWatchedTasks } = await import("../src/core/watchedTaskFollower.js");
    const { inboxRowHtml } = await import("../src/core/inbox.js");
    const { patchList } = await import("../src/core/patchList.js");
    follower = followWatchedTasks(); follower.follow([project]);
    await vi.waitFor(() => expect(follower.entries()[0]?.unreadCount).toBe(3), WAIT);
    const oldClone = { ...follower.entries()[0] };
    expect((await note())?.task_unread_display?.inbox).toBeUndefined();
    await publishActualList(5);
    await vi.waitFor(() => expect(follower.entries()[0]?.unreadCount).toBe(5), WAIT);
    patchList(document.querySelector("#consumer-row"), [oldClone], { keyOf: (entry) => entry.key,
      render: (entry) => follower.renderEntry(entry, () => inboxRowHtml(entry)) });
    expect(document.querySelector("#consumer-row .inbox-status-unread")).not.toBeNull();
    await sourceReadbacks();
    expect((await note())?.task_unread_display?.inbox).toBeUndefined();
    const actions = await import("../src/core/taskWaitingOnCache.js");
    const task = full({ title: "UNKNOWN after stale clone", read_through: "tc-20", updated_at: "2026-10-30T12:00:20Z" });
    for (const key of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete task[key];
    const read = await content.captureTaskContentRead(scope);
    expect(await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task },
      { contentRead: read, issuedRead: read.readOrder })).toBe(true);
    const listed = (await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0];
    expect(listed).not.toHaveProperty("unread_count");
    expect(listed.__task_family_cache?.unknown?.total).toBeDefined();
    await vi.waitFor(() => expect(follower.entries()[0]?.title).toBe("UNKNOWN after stale clone"), WAIT);
    expect(follower.entries()[0].unreadCount).toBe(0);
  });

  it("keeps the prior visible Inbox persist eligible on fold alone while hidden callbacks create no new persist", async () => {
    await seedActualInputs(); const writes = delayActualDisplayMerges();
    await mountActualInbox("projects");
    await vi.waitFor(() => expect(writes.captured).toBeGreaterThan(0), WAIT);
    const block = inboxRow().closest(".inbox-project");
    expect(block.classList.contains("inbox-folded")).toBe(false);
    await sourceReadbacks(); const before = writes.captured;
    block.querySelector("[data-project-fold]").click();
    await vi.waitFor(() => expect(block.classList.contains("inbox-folded")).toBe(true), WAIT);
    await sourceReadbacks();
    expect(writes.captured).toBe(before);
    await writes.release();
    expect((await note())?.task_unread_display?.inbox?.total).toBe(3);
    expect(block.classList.contains("inbox-folded")).toBe(true);
  });
});
