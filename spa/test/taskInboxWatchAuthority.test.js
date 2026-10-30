// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";
import { fakeSession } from "./deviceSessionFixture.js";

const deviceId = "inbox-shared-watch", projectId = "proj-1", taskId = "task-watch";
const html = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
let runtime;

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = html;
});
afterEach(() => {
  runtime?.inbox.unmountInboxList();
  runtime?.feed.stopFeed();
  runtime?.contexts.resetDeviceContexts();
  vi.restoreAllMocks();
});

async function mountInbox() {
  const { App } = await import("../src/app.js");
  Object.assign(App, { route: { name: "inbox" }, devices: [{ id: deviceId, name: "Laptop", status: "online" }],
    selectedDeviceId: deviceId, deviceFilter: null });
  runtime = {
    cache: await import("../src/core/localCache.js"),
    tracker: await import("../src/core/trackerCache.js"),
    content: await import("../src/core/taskContentAuthority.js"),
    fence: await import("../src/core/taskMutationFence.js"),
    inbox: await import("../src/core/inboxView.js"),
    feed: await import("../src/core/taskFeed.js"),
    contexts: await import("../src/core/deviceContexts.js"),
  };
  const project = { id: projectId, project_id: projectId, name: "Project", deviceId, projectKey: `${deviceId}|${projectId}` };
  await runtime.cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { items: [], runs: [], projects: [project], workspaces: [] });
  await runtime.cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [project]);
  await runtime.cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, []);
  const row = task({ id: taskId, project_id: projectId, number: 42, title: "Watched task", watched: true,
    status: "in_review", updated_at: new Date().toISOString(), trackers: [], read_through: "tc-10" });
  const address = runtime.tracker.tasksAddress(deviceId, projectId);
  const contentRead = await runtime.content.captureTaskContentRead(address, { revalidate: "full" });
  await runtime.tracker.writeTasksRecord(deviceId, projectId, runtime.tracker.tasksRecord([
    runtime.content.observeTaskContent(row, { contentRead, coverage: "full" }),
  ], [], contentRead.readOrder));
  const pending = deferred(), session = fakeSession(deviceId);
  session.call.mockImplementation((method) => method === "tasks.unwatch" ? pending.promise : new Promise(() => {}));
  const context = runtime.contexts.adoptDeviceSession(session);
  runtime.contexts.adoptBridgeSelection(context, { version: "3.27.0" }, { capabilities: { tasks: { watching: true } } });
  runtime.inbox.setInboxView("inbox");
  runtime.inbox.mountInboxList();
  await runtime.feed.startFeed();
  const control = () => document.querySelector('[data-unwatch="tracker_task:task-watch"]');
  await vi.waitFor(() => expect(control()).not.toBeNull());
  return { row, address, pending, session, control };
}

it("keeps an optimistic inbox unwatch across a pending read from an independent cache connection", async () => {
  const databases = new Set(), original = IDBDatabase.prototype.transaction;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    databases.add(this);
    return original.apply(this, args);
  });
  const { row, address, pending, session, control } = await mountInbox();
  control().click();
  await vi.waitFor(() => expect(session.call).toHaveBeenCalledWith("tasks.unwatch", { task_id: taskId }));
  expect(runtime.fence.taskMutationsPending(deviceId)).toBeGreaterThan(0);
  expect((await runtime.tracker.readTasksRecord(deviceId, projectId)).tasks[0].watched).toBe(false);
  const firstConnections = new Set(databases);

  // This reader has its own module-local mutation state and database handle;
  // only persisted shared authority can protect it from the pending mutation.
  vi.resetModules();
  const outside = {
    content: await import("../src/core/taskContentAuthority.js"),
    tracker: await import("../src/core/trackerCache.js"),
    fence: await import("../src/core/taskMutationFence.js"),
  };
  expect(outside.fence.taskMutationsPending(deviceId)).toBe(0);
  const contentRead = await outside.content.captureTaskContentRead(address, { revalidate: "full" });
  await outside.tracker.writeTasksRecord(deviceId, projectId, outside.tracker.tasksRecord([
    outside.content.observeTaskContent({ ...row, title: "Independent server observation" }, { contentRead, coverage: "full" }),
  ], [], contentRead.readOrder));
  expect([...databases].some((database) => !firstConnections.has(database))).toBe(true);
  const held = (await outside.tracker.readTasksRecord(deviceId, projectId)).tasks[0];
  expect(held.title).toBe("Independent server observation");
  expect(held.watched).toBe(false);

  pending.resolve({ task: { ...row, watched: false } });
  await vi.waitFor(() => expect(runtime.fence.taskMutationsPending(deviceId)).toBe(0));
});

it("contains a refused inbox owner registration and sends no unwatch RPC", async () => {
  const mounted = await mountInbox();
  const watch = await import("../src/core/taskWatchAuthority.js");
  const notices = await import("../src/core/notify.js");
  const begin = vi.spyOn(watch, "beginTaskWatchMutation").mockResolvedValue(null);
  const notice = vi.spyOn(notices, "notifyError").mockImplementation(() => {});
  mounted.control().click();
  await vi.waitFor(() => expect(begin).toHaveBeenCalledTimes(1));
  await vi.waitFor(() => expect(notice).toHaveBeenCalledWith(expect.stringContaining("Couldn't stop watching"), expect.any(String)));
  expect(runtime.fence.taskMutationsPending(deviceId)).toBe(0);
  expect(mounted.session.call.mock.calls.filter(([method]) => method === "tasks.unwatch")).toHaveLength(0);
  expect((await runtime.tracker.readTasksRecord(deviceId, projectId)).tasks[0].watched).toBe(true);
  expect(mounted.control()).not.toBeNull();
});

it.each(["foreign task", "foreign project", "null task", "opposite watch", "missing body"])(
  "rolls back and reports an inbox acknowledgment with %s instead of a matching full task", async (kind) => {
    const mounted = await mountInbox();
    const notices = await import("../src/core/notify.js");
    const watch = await import("../src/core/taskWatchAuthority.js");
    const notice = vi.spyOn(notices, "notifyError").mockImplementation(() => {});
    mounted.control().click();
    await vi.waitFor(() => expect(mounted.session.call).toHaveBeenCalledWith("tasks.unwatch", { task_id: taskId }));
    const answer = { ...mounted.row, watched: false };
    if (kind === "foreign task") answer.id = "foreign-task";
    if (kind === "foreign project") answer.project_id = "foreign-project";
    if (kind === "opposite watch") answer.watched = true;
    if (kind === "missing body") delete answer.body;
    mounted.pending.resolve({ task: kind === "null task" ? null : answer });
    await vi.waitFor(() => expect(runtime.fence.taskMutationsPending(deviceId)).toBe(0));
    expect(notice).toHaveBeenCalledWith(expect.stringContaining("Couldn't stop watching"), expect.any(String));
    expect((await runtime.tracker.readTasksRecord(deviceId, projectId)).tasks[0].watched).toBe(true);
    expect((await watch.readTaskWatchState({ deviceId, projectId })).pendingCount).toBe(0);
    expect(mounted.session.call.mock.calls.filter(([method]) => method === "tasks.unwatch")).toHaveLength(1);
  });

it("retains the legacy no-task inbox acknowledgment without fabricating a full task receipt", async () => {
  const mounted = await mountInbox();
  const notices = await import("../src/core/notify.js");
  const watch = await import("../src/core/taskWatchAuthority.js");
  const notice = vi.spyOn(notices, "notifyError").mockImplementation(() => {});
  mounted.control().click();
  await vi.waitFor(() => expect(mounted.session.call).toHaveBeenCalledWith("tasks.unwatch", { task_id: taskId }));
  mounted.pending.resolve({});
  await vi.waitFor(() => expect(runtime.fence.taskMutationsPending(deviceId)).toBe(0));
  expect(notice).not.toHaveBeenCalled();
  expect((await runtime.tracker.readTasksRecord(deviceId, projectId)).tasks[0].watched).toBe(false);
  const shared = await watch.readTaskWatchState({ deviceId, projectId });
  expect(shared.pendingCount).toBe(0);
  expect(shared.uncertain).toBe(true);
  expect(shared.eligible).toBe(false);
});
