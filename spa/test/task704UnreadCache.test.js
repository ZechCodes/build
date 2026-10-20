/** @vitest-environment jsdom */
// Real cache and task followers, with only the task page's wire boundary
// scripted. No session, polling, or server read is needed to paint unread.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, event, task, taskDetail } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {},
  bridgeCapabilities: () => ({
    changes: { subscriptions: true, kinds: ["tasks"] },
    tasks: { watching: true, attachments: true },
  }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({
    away: () => true,
    reconnecting: () => false,
    moved: () => () => {},
  }),
}));

const DEVICE = "task704-device";
const PROJECT = "task704-project";
const TASK = "task-704";
const project = { id: PROJECT, deviceId: DEVICE, projectKey: `${DEVICE}|${PROJECT}`, name: "Build" };
const first = "tc-01M5D3M1SY8D2VKESWJ87D9501";
const second = "tc-01M5D3M1SY8D2VKESWJ87D9502";
const watchedAt = "te-01M5D3M1SY8D2VKESWJ87D9503";
const newest = "tc-01M5D3M1SY8D2VKESWJ87D9504";
const stamp = (minute) => `2026-10-20T12:${String(minute).padStart(2, "0")}:00Z`;
const agentSays = (id, minute) => comment({
  id, task_id: TASK, author: { kind: "agent", agent_id: "task704-agent" },
  created_at: stamp(minute), body: `Agent update ${minute}`,
});
const watched = (over = {}) => task({
  id: TASK, project_id: PROJECT, number: 704, watched: true, status: "in_progress",
  created_at: stamp(0), updated_at: stamp(2), read_through: first,
  unread_count: 1, ...over,
});
const timeline = () => [agentSays(first, 1), agentSays(second, 2)];
const WAIT = { timeout: 3000 };

let modules;
let page;
let inbox;
let badge;
let host;
let focused;
let call;

async function importReaders() {
  modules = {
    tracker: await import("../src/core/trackerCache.js"),
    watched: await import("../src/core/watchedTaskFollower.js"),
    project: await import("../src/core/projectTasksUnread.js"),
    page: await import("../src/core/trackerTaskPage.js"),
  };
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="task704"></div>';
  host = document.querySelector("#task704");
  focused = vi.spyOn(document, "hasFocus").mockReturnValue(false);
  await importReaders();
  call = vi.fn((method, params) => method === "tasks.read_through"
    ? Promise.resolve({ task: { read_through: params.event_id } })
    : new Promise(() => {}));
});

function disposeReaders() {
  page?.dispose();
  inbox?.dispose();
  badge?.dispose();
  page = null;
  inbox = null;
  badge = null;
}

afterEach(() => {
  disposeReaders();
  vi.restoreAllMocks();
});

async function seed(one = watched(), rows = timeline()) {
  await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([one], columns()));
  await modules.tracker.writeTaskRecord(DEVICE, PROJECT, TASK, taskDetail(one, rows));
}

function follow() {
  inbox = modules.watched.followWatchedTasks();
  inbox.follow([project]);
}

const unread = () => inbox.entries().find((entry) => entry.taskId === TASK)?.unreadCount;
const readCalls = () => call.mock.calls.filter(([method]) => method === "tasks.read_through");

function openTask() {
  page = modules.page.mountTaskPage(host, {
    projectId: PROJECT, deviceId: DEVICE, projectKey: project.projectKey, taskId: TASK,
    callRpc: call, catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ projects: [project], workspaces: [], items: [] }), navigate: vi.fn(),
  });
}

it("shows a watched agent comment from the refreshed list before its detail arrives", async () => {
  const read = watched({ unread_count: 0, read_through: first, updated_at: stamp(1) });
  await seed(read, [agentSays(first, 1)]);
  follow();
  await vi.waitFor(() => expect(unread()).toBe(0), WAIT);

  const updated = watched();
  await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([updated], columns()));
  await vi.waitFor(() => expect(unread()).toBe(1), WAIT);
  await modules.tracker.writeTaskRecord(DEVICE, PROJECT, TASK, taskDetail(updated, timeline()));
  await vi.waitFor(() => expect(unread()).toBe(1), WAIT);
  expect(call).not.toHaveBeenCalled();
});

it("clears Inbox only after the visible task's read report is accepted", async () => {
  await seed();
  follow();
  openTask();
  await vi.waitFor(() => expect(host.querySelector(".task-page-title")).not.toBeNull(), WAIT);
  await vi.waitFor(() => expect(unread()).toBe(1), WAIT);
  expect(readCalls()).toHaveLength(0);

  let accept;
  call.mockImplementation((method, params) => method === "tasks.read_through"
    ? new Promise((resolve) => { accept = () => resolve({ task: { read_through: params.event_id } }); })
    : new Promise(() => {}));
  focused.mockReturnValue(true);
  window.dispatchEvent(new Event("focus"));
  await vi.waitFor(() => expect(readCalls()).toHaveLength(1), WAIT);
  expect(unread()).toBe(1);
  accept();
  await vi.waitFor(() => expect(unread()).toBe(0), WAIT);
});

it("retains the accepted cursor through a cache reader restart and an older detail reply", async () => {
  await seed();
  follow();
  focused.mockReturnValue(true);
  openTask();
  await vi.waitFor(() => expect(unread()).toBe(0), WAIT);
  expect(readCalls()).toHaveLength(1);
  disposeReaders();

  // Recreate the modules and their in-memory followers, retaining the same
  // IndexedDB storage just as a page reload does.
  vi.resetModules();
  await importReaders();
  follow();
  await vi.waitFor(() => expect(unread()).toBe(0), WAIT);
  expect((await modules.tracker.readTaskRecord(DEVICE, PROJECT, TASK)).task.read_through).toBe(second);
  await modules.tracker.writeTaskRecord(DEVICE, PROJECT, TASK, taskDetail(watched(), timeline()));
  expect((await modules.tracker.readTaskRecord(DEVICE, PROJECT, TASK)).task.read_through).toBe(second);
  await vi.waitFor(() => expect(unread()).toBe(0), WAIT);
});

it("clears a passive Inbox when another client's accepted read reaches the task list cache", async () => {
  await seed();
  follow();
  await vi.waitFor(() => expect(unread()).toBe(1), WAIT);

  // The tasks push is an invalidation. Its tasks.list response carries the
  // remote client's cursor, while this tab still holds the earlier detail.
  const remotelyRead = watched({ read_through: second, unread_count: 0, updated_at: stamp(3) });
  await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([remotelyRead], columns()));
  await vi.waitFor(() => expect(unread()).toBe(0), WAIT);
  expect((await modules.tracker.readTaskRecord(DEVICE, PROJECT, TASK)).task.read_through).toBe(first);
  expect(call).not.toHaveBeenCalled();
});

it("clears the project Tasks badge together with Inbox before the task list refreshes", async () => {
  await seed();
  follow();
  badge = modules.project.followProjectTasksUnread();
  badge.follow(DEVICE, PROJECT);
  await vi.waitFor(() => expect(badge.count()).toBe(1), WAIT);
  focused.mockReturnValue(true);
  openTask();
  await vi.waitFor(() => expect(unread()).toBe(0), WAIT);
  await vi.waitFor(() => expect(badge.count()).toBe(0), WAIT);
  expect((await modules.tracker.readTasksRecord(DEVICE, PROJECT)).tasks[0].read_through).toBe(first);
});

it.each(["persisted", "legacy"])("starts New after the %s watch baseline, matching Inbox", async (baseline) => {
  const one = watched({
    updated_at: stamp(4), read_through: null,
    ...(baseline === "persisted" ? { watch_started_after: watchedAt } : {}),
  });
  await seed(one, [
    ...timeline(),
    event({ id: watchedAt, task_id: TASK, kind: "watched", at: stamp(3) }),
    agentSays(newest, 4),
  ]);
  follow();
  openTask();
  await vi.waitFor(() => expect(unread()).toBe(1), WAIT);
  await vi.waitFor(() => expect(host.querySelector(".task-unread-line")).not.toBeNull(), WAIT);
  expect(host.querySelector(".task-unread-line").nextElementSibling.id).toBe(`comment-${newest}`);
  expect(readCalls()).toHaveLength(0);
});
