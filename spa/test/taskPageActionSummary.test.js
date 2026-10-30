// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBObjectStore } from "fake-indexeddb";
import { columns, comment, task } from "./trackerWireFixture.js";
import { readCached, wipeCache } from "../src/core/localCache.js";
import { readTaskRecord, writeTaskRecord, writeTasksRecord } from "../src/core/trackerCache.js";
import { beginSummaryQuery, landSummaryPage, readSummaryQuery, summaryAddress } from "../src/core/taskSummaryCache.js";
import { nextTaskRead } from "../src/core/taskReadOrder.js";
import { taskMutationsPending } from "../src/core/taskMutationFence.js";
import { adoptBridgeSelection, adoptDeviceSession, resetDeviceContexts, retireDeviceContext } from "../src/core/deviceContexts.js";
import { createTaskWaitingOnActions } from "../src/core/taskWaitingOnActions.js";
import { mountTaskPage } from "../src/core/trackerTaskPage.js";
import { fakeSession } from "./deviceSessionFixture.js";

const observation = vi.hoisted(() => ({ carries: false, session: null }));
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {}, disarmChangeEvents() {},
  bridgeCapabilities: () => ({ tasks: { waitingOn: true }, changes: { subscriptions: true, kinds: ["tasks"] } }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => observation.session,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/trackerWatch.js", async (original) => ({ ...await original(), carriesWatching: () => observation.carries }));
vi.mock("../src/core/notify.js", () => ({ notifyError: vi.fn() }));

const scope = { deviceId: "task-page-action-900", projectId: "proj-1" }, taskId = "task-900";
const timeline = [comment({ id: "tc-04", task_id: taskId, body: "Keep this full timeline", created_at: "2026-10-29T12:00:00Z" })];
const full = (extra = {}) => task({ id: taskId, project_id: scope.projectId, number: 12, title: "Before", watched: false,
  trackers: [], updated_at: "2026-10-29T12:00:00Z", read_through: "tc-01", body: "Keep full body", ...extra });
const normalized = (extra = {}) => ({ id: taskId, project_id: scope.projectId, number: 12, title: "Before", state: "open",
  status: "ready", watched: false, read_through: "tc-01", updated_at: "2026-10-29T12:00:00Z", ...extra });
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
let session, context, pages, ticket;
const cachedSummary = async () => (await readCached(summaryAddress(scope, taskId)))?.value;
const mount = async () => {
  const host = document.createElement("div"); document.body.append(host);
  const page = mountTaskPage(host, { ...scope, taskId, projectKey: `${scope.deviceId}|${scope.projectId}`, callRpc: context.rpc,
    catalog: () => ({ providers: [] }), feed: () => ({ items: [], projects: [], workspaces: [] }) });
  pages.push(page);
  await vi.waitFor(() => expect(host.querySelector(".rail-watch")).not.toBeNull());
  return { host, page, button: () => host.querySelector(".rail-watch") };
};
const count = (method) => session.call.mock.calls.filter(([name]) => name === method).length;
const settle = async () => vi.waitFor(() => expect(taskMutationsPending(scope.deviceId)).toBe(0));
const replacementSession = () => {
  const replacement = fakeSession(scope.deviceId);
  replacement.call.mockImplementation((method) => method === "tasks.get" ? Promise.resolve({ task: full(), timeline }) : Promise.resolve({}));
  return replacement;
};

beforeEach(async () => {
  resetDeviceContexts(); await wipeCache(); document.body.innerHTML = ""; pages = [];
  observation.carries = false;
  session = fakeSession(scope.deviceId); observation.session = session; context = adoptDeviceSession(session);
  adoptBridgeSelection(context, { version: "3.27.0" }, { capabilities: { tasks: { waitingOn: true } } });
  session.call.mockImplementation((method) => method === "tasks.get" ? Promise.resolve({ task: full(), timeline }) : Promise.resolve({}));
  await writeTaskRecord(scope.deviceId, scope.projectId, taskId, { task: full(), timeline });
  await writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [full()], columns: columns() });
  const read = await nextTaskRead(); ticket = await beginSummaryQuery(scope, {}, read);
  await landSummaryPage(ticket, { summary: true, basis: "opaque-before", tasks: [normalized()], next_cursor: "keep-cursor" }, { read });
});
afterEach(() => { pages.forEach((page) => page.dispose()); resetDeviceContexts(); vi.restoreAllMocks(); });

it("projects a full watch acknowledgment into normalized summaries while retaining body and timeline only in detail", async () => {
  let serverCanonicalTask = full();
  const pending = deferred(); session.call.mockImplementation((method) => method === "tasks.get"
    ? Promise.resolve({ task: structuredClone(serverCanonicalTask), timeline }) : pending.promise);
  const mounted = await mount(); mounted.button().click();
  await vi.waitFor(() => expect(count("tasks.watch")).toBe(1));
  serverCanonicalTask = full({ watched: true, watch_started_after: "", unread_count: 0,
    read_through: "tc-04\0", updated_at: "2026-10-29T12:01:00Z", attachments: [{ path: "large-file" }] });
  pending.resolve({ task: structuredClone(serverCanonicalTask) });
  await settle();
  const summary = await cachedSummary();
  expect(summary.task).toMatchObject({ watched: true, watch_started_after: "", read_through: "tc-04\0", unread_count: 0, attachment_count: 1 });
  expect(summary.task).not.toHaveProperty("body"); expect(summary.task).not.toHaveProperty("attachments");
  expect(summary.needs_unread_refresh).toBe(true);
  const detail = await readTaskRecord(scope.deviceId, scope.projectId, taskId);
  expect(detail.task.body).toBe("Keep full body"); expect(detail.timeline).toEqual(timeline);
  expect((await readSummaryQuery(scope, {})).stale).toBe(true);
  expect(await landSummaryPage({ ...ticket, cursor: "keep-cursor" }, { summary: true, basis: "opaque-before", tasks: [] }, { read: await nextTaskRead() })).toBe(false);
});

it("projects full read acknowledgments without deriving missing attention counters or replacing the timeline", async () => {
  observation.carries = true;
  const pending = deferred(); session.call.mockImplementation((method) => method === "tasks.get"
    ? Promise.resolve({ task: full(), timeline }) : pending.promise);
  await mount(); await vi.waitFor(() => expect(count("tasks.read_through")).toBe(1));
  pending.resolve({ task: full({ watched: true, read_through: "tc-05\0", watch_started_after: "", unread_count: 0 }) });
  await settle();
  const summary = await cachedSummary();
  expect(summary.task).toMatchObject({ watched: true, read_through: "tc-05\0", watch_started_after: "", unread_count: 0 });
  expect(summary.task).not.toHaveProperty("unread_mentions"); expect(summary.task).not.toHaveProperty("unread_agent_comments");
  expect(summary.needs_unread_refresh).toBe(true);
  expect((await readTaskRecord(scope.deviceId, scope.projectId, taskId)).timeline).toEqual(timeline);
});

it("does not let an older same-timestamp watch reply reverse a newer read action", async () => {
  const watch = deferred(), read = deferred();
  session.call.mockImplementation((method) => method === "tasks.get" ? Promise.resolve({ task: full(), timeline })
    : method === "tasks.watch" ? watch.promise : read.promise);
  const first = await mount(); first.button().click(); await vi.waitFor(() => expect(count("tasks.watch")).toBe(1));
  first.page.dispose(); first.host.remove(); pages = [];
  observation.carries = true; await mount(); await vi.waitFor(() => expect(count("tasks.read_through")).toBe(1));
  read.resolve({ task: full({ watched: false, read_through: "tc-20" }) });
  await vi.waitFor(async () => expect((await cachedSummary()).task.read_through).toBe("tc-20"));
  watch.resolve({ task: full({ watched: true, read_through: "tc-10", watch_started_after: "", unread_count: 4 }) });
  await settle();
  expect((await cachedSummary()).task).toMatchObject({ watched: false, read_through: "tc-20" });
  expect((await readTaskRecord(scope.deviceId, scope.projectId, taskId)).task).toMatchObject({ watched: false, read_through: "tc-20" });
});

it.each(["session", "context", "account"])("rejects a full acknowledgment after %s authority replacement", async (kind) => {
  const pending = deferred(); session.call.mockImplementation((method) => method === "tasks.get"
    ? Promise.resolve({ task: full(), timeline }) : pending.promise);
  const mounted = await mount(); mounted.button().click(); await vi.waitFor(() => expect(count("tasks.watch")).toBe(1));
  const before = await cachedSummary();
  if (kind === "context") retireDeviceContext(scope.deviceId);
  if (kind === "account") resetDeviceContexts();
  const replacement = replacementSession(); observation.session = replacement; adoptDeviceSession(replacement);
  pending.resolve({ task: full({ watched: true, title: "Obsolete" }) });
  await settle();
  expect(await cachedSummary()).toEqual(before);
});

it("keeps acknowledged cache effects after the originating page is disposed", async () => {
  const pending = deferred(); session.call.mockImplementation((method) => method === "tasks.get"
    ? Promise.resolve({ task: full(), timeline }) : pending.promise);
  const mounted = await mount(); mounted.button().click(); await vi.waitFor(() => expect(count("tasks.watch")).toBe(1));
  mounted.page.dispose(); pages = [];
  pending.resolve({ task: full({ watched: true, title: "Accepted after leaving" }) });
  await settle(); expect((await cachedSummary()).task).toMatchObject({ watched: true, title: "Accepted after leaving" });
});

it("rejects a full acknowledgment whose transaction waits through a session retarget", async () => {
  const pending = deferred(); session.call.mockImplementation((method) => method === "tasks.get"
    ? Promise.resolve({ task: full(), timeline }) : pending.promise);
  const mounted = await mount(); mounted.button().click(); await vi.waitFor(() => expect(count("tasks.watch")).toBe(1));
  const before = await cachedSummary(), original = IDBObjectStore.prototype.get;
  let retargeted = false;
  vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
    if (!retargeted && String(key).includes("|tracker-summary|")) {
      retargeted = true;
      const replacement = replacementSession(); observation.session = replacement; adoptDeviceSession(replacement);
    }
    return original.call(this, key);
  });
  pending.resolve({ task: full({ watched: true, title: "Rejected in transaction" }) });
  await settle(); expect(retargeted).toBe(true); expect(await cachedSummary()).toEqual(before);
});

it("checks fallback read-floor acceptance inside a delayed cache transaction", async () => {
  observation.carries = true;
  const pending = deferred(); session.call.mockImplementation((method) => method === "tasks.get"
    ? Promise.resolve({ task: full(), timeline }) : pending.promise);
  await mount(); await vi.waitFor(() => expect(count("tasks.read_through")).toBe(1));
  const original = IDBObjectStore.prototype.get;
  let retargeted = false;
  vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
    if (!retargeted && String(key).includes("|tracker-task|")) {
      retargeted = true;
      const replacement = replacementSession(); observation.session = replacement; adoptDeviceSession(replacement);
    }
    return original.call(this, key);
  });
  pending.resolve({}); await settle();
  expect(retargeted).toBe(true);
  expect((await readTaskRecord(scope.deviceId, scope.projectId, taskId)).task.read_through).toBe("tc-01");
});

it("keeps newer detail body/title/timeline while an older full read acknowledgment advances its floor", async () => {
  observation.carries = true;
  const pending = deferred(), newer = full({ title: "Newer title", body: "Newer body", watched: true,
    updated_at: "2026-10-29T13:00:00Z", watch_started_after: "" });
  await writeTaskRecord(scope.deviceId, scope.projectId, taskId, { task: newer, timeline });
  const read = await nextTaskRead(), query = await beginSummaryQuery(scope, {}, read);
  await landSummaryPage(query, { summary: true, basis: "newer-content", tasks: [normalized({ title: newer.title,
    updated_at: newer.updated_at, watched: true, watch_started_after: "" })] }, { read });
  session.call.mockImplementation((method) => method === "tasks.get" ? Promise.resolve({ task: newer, timeline }) : pending.promise);
  await mount(); await vi.waitFor(() => expect(count("tasks.read_through")).toBe(1));
  pending.resolve({ task: full({ title: "Old title", body: "Old body", watched: true, read_through: "tc-30\0", watch_started_after: "", unread_count: 0 }) });
  await settle();
  const detail = await readTaskRecord(scope.deviceId, scope.projectId, taskId);
  expect(detail.task).toMatchObject({ title: "Newer title", body: "Newer body", read_through: "tc-30\0" });
  expect(detail.timeline).toEqual(timeline);
  expect((await cachedSummary()).task).toMatchObject({ title: "Newer title", read_through: "tc-30\0" });
});

it.each([{ id: "foreign-task" }, { project_id: "foreign-project" }, { number: 13 }])(
  "rejects a full acknowledgment for a different captured task identity %#", async (patch) => {
    const pending = deferred(); session.call.mockImplementation((method) => method === "tasks.get"
      ? Promise.resolve({ task: full(), timeline }) : pending.promise);
    const mounted = await mount(); mounted.button().click(); await vi.waitFor(() => expect(count("tasks.watch")).toBe(1));
    const before = await cachedSummary();
    pending.resolve({ task: full({ watched: true, ...patch }) });
    await settle(); expect(await cachedSummary()).toEqual(before);
  },
);

it("refuses a malformed partial task acknowledgment instead of treating it as a legacy no-task reply", async () => {
  observation.carries = true;
  const pending = deferred(); session.call.mockImplementation((method) => method === "tasks.get"
    ? Promise.resolve({ task: full(), timeline }) : pending.promise);
  await mount(); await vi.waitFor(() => expect(count("tasks.read_through")).toBe(1));
  const before = await cachedSummary(), detail = await readTaskRecord(scope.deviceId, scope.projectId, taskId);
  pending.resolve({ task: { read_through: "tc-99" } }); await settle();
  expect(await cachedSummary()).toEqual(before);
  expect(await readTaskRecord(scope.deviceId, scope.projectId, taskId)).toEqual(detail);
});

it("does not let an older no-task read acknowledgment raise the floor after a newer full watch action", async () => {
  const read = deferred(), watch = deferred(), longer = [comment({ ...timeline[0], id: "tc-99" })];
  await writeTaskRecord(scope.deviceId, scope.projectId, taskId, { task: full(), timeline: longer });
  session.call.mockImplementation((method) => method === "tasks.get" ? Promise.resolve({ task: full(), timeline: longer })
    : method === "tasks.watch" ? watch.promise : read.promise);
  observation.carries = true;
  const first = await mount(); await vi.waitFor(() => expect(count("tasks.read_through")).toBe(1));
  first.page.dispose(); first.host.remove(); pages = [];
  observation.carries = false;
  const second = await mount(); second.button().click(); await vi.waitFor(() => expect(count("tasks.watch")).toBe(1));
  watch.resolve({ task: full({ watched: true, watch_started_after: "", read_through: "tc-20", unread_count: 0 }) });
  await vi.waitFor(async () => expect((await cachedSummary()).task.read_through).toBe("tc-20"));
  read.resolve({}); await settle();
  expect((await cachedSummary()).task.read_through).toBe("tc-20");
  expect((await readTaskRecord(scope.deviceId, scope.projectId, taskId)).task.read_through).toBe("tc-20");
});

it.each(["dependency", "assignment"])("rejects an older %s owner's full reply after a newer page unwatch", async (owner) => {
  const older = deferred(), unwatch = deferred(), watched = full({ watched: true, watch_started_after: "", unread_count: 0 });
  let serverCanonicalTask = watched;
  await writeTaskRecord(scope.deviceId, scope.projectId, taskId, { task: watched, timeline });
  // Each read captures server state when dispatched. In-flight reads retain
  // their pre-ack snapshot; recovery after the ack sees the committed unwatch.
  session.call.mockImplementation((method) => method === "tasks.get" ? Promise.resolve({ task: structuredClone(serverCanonicalTask), timeline })
    : method === "tasks.unwatch" ? unwatch.promise : older.promise);
  const actions = createTaskWaitingOnActions({ ...scope, taskId, call: context.rpc });
  const request = owner === "dependency" ? actions.add(["blocker"]) : actions.assign({ assignee: { kind: "user" } });
  const method = owner === "dependency" ? "tasks.waiting_on.add" : "tasks.assign";
  await vi.waitFor(() => expect(count(method)).toBe(1));
  const mounted = await mount(); expect(mounted.button().getAttribute("aria-pressed")).toBe("true");
  mounted.button().click(); await vi.waitFor(() => expect(count("tasks.unwatch")).toBe(1));
  serverCanonicalTask = full({ watched: false, read_through: "tc-20" });
  unwatch.resolve({ task: structuredClone(serverCanonicalTask) });
  await vi.waitFor(async () => expect((await cachedSummary()).task.read_through).toBe("tc-20"));
  older.resolve({ task: watched }); await request; await settle();
  expect((await cachedSummary()).task).toMatchObject({ watched: false, read_through: "tc-20" });
  expect((await readTaskRecord(scope.deviceId, scope.projectId, taskId)).task).toMatchObject({ watched: false, read_through: "tc-20" });
});
