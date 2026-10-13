/** @vitest-environment jsdom */
// Real board/list pickers, mutation cache writes, detail feed and attention.
// Only the bridge wire is scripted: a partial assignment must not make an
// old timeline permanently current or admit a detail read begun before it.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, task } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ tasks: { waitingOn: true }, changes: { kinds: ["tasks"] } }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => ({ adapter: { capabilities: { tasks: { waitingOn: true } } } }),
  whenGreeted: async (_context, send) => ({ sent: send(() => true), stands: () => true }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const DEVICE = "assignment-refresh";
const PROJECT = "project";
let pane, detailFeed;
beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="tasks"></div>';
});
afterEach(() => { pane?.dispose(); detailFeed?.dispose(); vi.restoreAllMocks(); });

it.each([["board", ""], ["list", ""], ["board", "none"], ["list", "none"]])("refreshes the %s picker partial response timeline after a fenced concurrent mention read (assignee filter %s)", async (view, assigneeFilter) => {
  vi.spyOn(Date, "now").mockReturnValue(123456);
  const tracker = await import("../src/core/trackerCache.js");
  const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
  const { rememberNeedsYouRule } = await import("../src/core/needsYouRule.js");
  const { uiAddress } = await import("../src/core/localUiState.js");
  const { writeUiRecord } = await import("../src/core/localUiStore.js");
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  const { taskUnreadCount } = await import("../src/core/taskUnread.js");
  const { taskAttention } = await import("../src/core/trackerAttentionModel.js");
  const old = task({ id: "mention-task", project_id: PROJECT, watched: true, unread_count: 0,
    title: "Concurrent mention", updated_at: "2026-10-13T12:00:00Z" });
  const mentioned = { ...old, updated_at: "2026-10-13T12:01:00Z", unread_count: 1 };
  const assigned = { ...mentioned, assignee: { kind: "project_agent" } };
  const mention = comment({ id: "tc-02", task_id: old.id, author: { kind: "agent", agent_id: "agent" }, mentions_user: true });
  await tracker.writeTasksRecord(DEVICE, PROJECT, tracker.tasksRecord([old], columns(), await nextTaskRead()));
  await tracker.writeTaskRecord(DEVICE, PROJECT, old.id, tracker.taskRecord(old, []));
  await rememberNeedsYouRule(DEVICE, { tasks: { commentUserNotifies: true } });
  await writeUiRecord(uiAddress({ deviceId: DEVICE, entityId: PROJECT, view: `tasks:${DEVICE}|${PROJECT}`, kind: "filters" }),
    { view, filters: { assignee: assigneeFilter } });
  const details = [];
  let assignmentLanded = false;
  const call = vi.fn((method, params) => {
    if (method === "tasks.columns") return Promise.resolve({ columns: columns() });
    if (method === "tasks.list") return Promise.resolve({ tasks: assignmentLanded && params.assignee === "none"
      ? [] : [assignmentLanded ? assigned : mentioned] });
    if (method === "tasks.get") return new Promise((resolve) => details.push(resolve));
    if (method === "tasks.assign") { assignmentLanded = true; return Promise.resolve({ task: assigned, dispatch: null }); }
    return Promise.resolve({});
  });
  const host = document.querySelector("#tasks");
  pane = mountTasksPane(host, { deviceId: DEVICE, projectId: PROJECT, projectKey: `${DEVICE}|${PROJECT}`,
    view, callRpc: call, catalog: () => ({ providers: [] }), feed: () => ({ items: [], workspaces: [] }) });
  await vi.waitFor(() => expect(details).toHaveLength(1));
  host.querySelector("[data-task-assign]").click();
  const select = document.querySelector("[data-assignee-select]");
  select.value = "project_agent";
  select.dispatchEvent(new Event("change", { bubbles: true }));
  document.querySelector("[data-assign-go]").click();
  await vi.waitFor(async () => expect((await tracker.readTaskRecord(DEVICE, PROJECT, old.id)).task.assignee).toEqual(assigned.assignee));
  const admission = vi.spyOn(tracker, "writeTaskRecord");
  details[0]({ task: mentioned, timeline: [mention] });
  if (!assigneeFilter) {
    await vi.waitFor(() => expect(admission).toHaveBeenCalled());
    await admission.mock.results[0].value;
  }
  // Same local clock: the mutation read fence, rather than its cache stamp,
  // must retain the assignment and reject the pre-response timeline.
  const fenced = await tracker.readTaskRecord(DEVICE, PROJECT, old.id);
  expect(fenced.task.assignee).toEqual(assigned.assignee);
  expect(fenced.timeline).toEqual([]);
  await vi.waitFor(() => expect(details).toHaveLength(2));
  details[1]({ task: assigned, timeline: [mention] });
  await vi.waitFor(async () => {
    const detail = await tracker.readTaskRecord(DEVICE, PROJECT, old.id);
    expect(taskUnreadCount(assigned, detail)).toBe(1);
    expect(taskAttention(assigned, { detail, askedOnly: true }).reasons).toContain("inbox_comment");
  });
  if (assigneeFilter) await vi.waitFor(() => expect(host.querySelector("[data-task-assign]")).toBeNull());
  else {
    await vi.waitFor(() => expect(host.querySelector(".task-unread")?.textContent).toBe("1"));
    if (view === "list") expect(host.querySelector('[data-task-group="needsYou"]')?.textContent).toContain("Concurrent mention");
  }
});

it("coalesces repeated mutation refreshes behind a pending detail read", async () => {
  const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
  const waiting = task({ id: "coalesced", project_id: PROJECT });
  const replies = [];
  const call = vi.fn(() => new Promise((resolve) => replies.push(resolve)));
  detailFeed = createTrackerTaskDetailsFeed({ deviceId: DEVICE, projectId: PROJECT, callRpc: call });
  await detailFeed.updateTasks([waiting]);
  await vi.waitFor(() => expect(replies).toHaveLength(1));
  const first = detailFeed.refresh(waiting.id);
  const second = detailFeed.refresh(waiting.id);
  expect(call).toHaveBeenCalledTimes(1);
  replies[0]({ task: waiting, timeline: [] });
  await vi.waitFor(() => expect(replies).toHaveLength(2));
  replies[1]({ task: waiting, timeline: [comment({ id: "tc-02" })] });
  await Promise.all([first, second]);
  await vi.waitFor(() => expect(detailFeed.read().get(waiting.id)?.timeline?.[0]?.id).toBe("tc-02"));
  expect(call).toHaveBeenCalledTimes(2);
});

it("keeps a mutation refresh queued at the four-read bound even when a partial response makes its task current", async () => {
  const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
  const tracker = await import("../src/core/trackerCache.js");
  const tasks = Array.from({ length: 5 }, (_, index) => task({ id: `bounded-${index}`, project_id: PROJECT }));
  const replies = [];
  const call = vi.fn((_method, params) => new Promise((resolve) => replies.push({ id: params.task_id, resolve })));
  detailFeed = createTrackerTaskDetailsFeed({ deviceId: DEVICE, projectId: PROJECT, callRpc: call });
  await detailFeed.updateTasks(tasks);
  await vi.waitFor(() => expect(replies).toHaveLength(4));
  detailFeed.refresh(tasks[4].id);
  await tracker.writeTaskRecord(DEVICE, PROJECT, tasks[4].id, tracker.taskRecord(tasks[4], []));
  await vi.waitFor(() => expect(detailFeed.read().has(tasks[4].id)).toBe(true));
  expect(call).toHaveBeenCalledTimes(4);
  replies[0].resolve({ task: tasks[0], timeline: [] });
  await vi.waitFor(() => expect(replies).toHaveLength(5));
  expect(replies[4].id).toBe(tasks[4].id);
  replies[4].resolve({ task: tasks[4], timeline: [comment({ id: "tc-02" })] });
  await vi.waitFor(() => expect(detailFeed.read().get(tasks[4].id)?.timeline?.[0]?.id).toBe("tc-02"));
});
