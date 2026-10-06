// @vitest-environment jsdom
// #125: a watched task's inbox row paints from the cache on a reload, before
// the bridge answers anything, and Stop watching takes it away at once and
// puts it back if the bridge refuses. Real cache, feed, device registry and
// rail; the session's `call` is the only stand-in. And (#144) a cold reload
// with no session at all still paints the row from what the cache holds, and
// Stop watching takes a question out of the Dashboard's Needs you as well.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { comment, task, taskDetail } from "./trackerWireFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const DEVICE = "watch-device";
const PROJECT = "proj-1";
const WAIT = { timeout: 5000, interval: 20 };
const GREETING = {
  api_version: "2.0.0",
  push_events: true,
  capabilities: ["tasks.watching", "tasks.attachments"],
  tasks: { watching: true, attachments: true },
};
const project = { id: PROJECT, project_id: PROJECT, name: "Build", deviceId: DEVICE, projectKey: `${DEVICE}|${PROJECT}` };
const review = task({ id: "task-7", number: 7, title: "Wire 1.22", watched: true, status: "in_review", updated_at: "2026-09-24T01:00:00Z" });

let modules, answers;
const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")];
const rowFor = (id) => rows().find((row) => row.dataset.key === `tracker_task:${id}`) || null;

/** The bridge: greets, and answers what a case scripted; anything else never
 *  answers, so what paints came from the cache. */
let greeting = GREETING;
const call = vi.fn((method, params) => {
  if (method === "session.hello") return Promise.resolve(greeting);
  const answer = answers[method];
  return answer ? answer(params) : new Promise(() => {});
});

/** A reload: the feed and the task records are already on disk. `greet`
 *  lands a session before the rail mounts; without it no machine answers. */
async function boot({ greet = true, tasks = [review], rule = null, hello = GREETING, timelines = {}, agentRows = [], namesMakers = false, projectEntityId = null } = {}) {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  answers = {};
  greeting = hello;
  call.mockClear();
  const { App } = await import("../src/app.js");
  Object.assign(App, { route: { name: "inbox" }, devices: [{ id: DEVICE, name: "Laptop", status: "online" }],
    selectedDeviceId: DEVICE, deviceFilter: null });
  modules = {
    cache: await import("../src/core/localCache.js"),
    tracker: await import("../src/core/trackerCache.js"),
    taskFeed: await import("../src/core/taskFeed.js"),
    inboxView: await import("../src/core/inboxView.js"),
    deviceContexts: await import("../src/core/deviceContexts.js"),
    connection: await import("../src/connection.js"),
    needsYouRule: await import("../src/core/needsYouRule.js"),
    tasksPane: await import("../src/core/trackerTasksPane.js"),
  };
  const address = (kind) => ({ deviceId: DEVICE, entityId: "", kind });
  const listedProject = { ...project, entity_id: projectEntityId };
  await modules.cache.writeCached(address("feed"), { items: [], runs: [], projects: [listedProject], workspaces: [] });
  await modules.cache.writeCached(address("projects"), [listedProject]);
  await modules.cache.writeCached(address("workspaces"), []);
  await modules.cache.writeCached(address("agent-lineage-support"), { namesMakers });
  for (const row of agentRows) await modules.cache.writeCached({ deviceId: DEVICE, entityId: row.run_id, kind: "row" }, row);
  await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord(tasks, []));
  for (const one of tasks) await modules.tracker.writeTaskRecord(DEVICE, PROJECT, one.id, taskDetail(one, timelines[one.id] || []));
  if (rule) await modules.needsYouRule.rememberNeedsYouRule(DEVICE, rule);
  if (greet) {
    const context = modules.deviceContexts.adoptDeviceSession({
      deviceId: DEVICE, call, close: () => {}, peer: () => {}, onCarrier: () => {},
      installAdapter: (selection) => selection.create(call),
    });
    await modules.connection.greetLiveBridge(context);
  }
  modules.inboxView.setInboxView("inbox");
  modules.inboxView.mountInboxList();
  await modules.taskFeed.startFeed();
}

afterEach(() => {
  modules.inboxView.unmountInboxList();
  modules.taskFeed.stopFeed();
  modules.deviceContexts.resetDeviceContexts();
});

describe("watched task dots from cached agents", () => {
  const assigned = task({ id: "task-running", watched: true, status: "in_review",
    assignee: { kind: "agent", agent_id: "agent-task" } });
  const agentRow = (agents) => ({ kind: "branch", project_id: PROJECT, run_id: "run-task", agents });
  const dotFor = () => rowFor(assigned.id)?.querySelector(".inbox-status-dot");
  const landAgents = (agents) => modules.cache.writeCached({ deviceId: DEVICE, entityId: "run-task", kind: "row" }, agentRow(agents));

  it("paints the assigned agent's running state before a greeting, then changes when cached work stops", async () => {
    await boot({ greet: false, tasks: [assigned], agentRows: [agentRow([{ id: "agent-task", working: true }])] });
    await vi.waitFor(() => expect(dotFor()?.classList.contains("inbox-status-running")).toBe(true), WAIT);
    expect(call).not.toHaveBeenCalled();
    await landAgents([{ id: "agent-task", working: false }]);
    await vi.waitFor(() => expect(dotFor()).toBe(null), WAIT);
    expect(rowFor(assigned.id)).not.toBe(null);
    expect(call).not.toHaveBeenCalled();
  });

  it("pulses while the assigned agent waits on a harness agent and stops when that child finishes", async () => {
    const parent = { id: "agent-task", working: false,
      surfaces: { subagents: [{ id: "harness-child", state: "running" }] } };
    await boot({ greet: false, tasks: [assigned], agentRows: [agentRow([parent])] });
    await vi.waitFor(() => expect(dotFor()?.classList.contains("inbox-status-running")).toBe(true), WAIT);
    await landAgents([{ ...parent, surfaces: { subagents: [{ id: "harness-child", state: "idle" }] } }]);
    await vi.waitFor(() => expect(dotFor()).toBe(null), WAIT);
    expect(rowFor(assigned.id)).not.toBe(null);
    expect(call).not.toHaveBeenCalled();
  });

  it("includes the assigned agent's running Build descendants when the cache names makers", async () => {
    const agents = [
      { id: "agent-task", working: false },
      { id: "agent-child", created_by: "agent-task", working: false },
      { id: "agent-grandchild", created_by: "agent-child", working: true },
    ];
    await boot({ greet: false, tasks: [assigned], agentRows: [agentRow(agents)], namesMakers: true });
    await vi.waitFor(() => expect(dotFor()?.classList.contains("inbox-status-running")).toBe(true), WAIT);
    await modules.cache.writeCached({ deviceId: DEVICE, entityId: "", kind: "agent-lineage-support" }, { namesMakers: false });
    await vi.waitFor(() => expect(dotFor()).toBe(null), WAIT);
    expect(call).not.toHaveBeenCalled();
  });

  it("folds a project's running watched task into its head even while the task has no Needs-you row", async () => {
    const quietTask = { ...assigned, status: "in_progress" };
    const mine = task({ id: "task-mine", watched: true, assignee: { kind: "user" } });
    await boot({ greet: false, tasks: [quietTask, mine], rule: { tasks: { commentUserNotifies: true } },
      agentRows: [agentRow([{ id: "agent-task", working: false,
        surfaces: { subagents: [{ id: "harness-child", state: "running" }] } }])] });
    modules.inboxView.setInboxView("projects");
    const head = () => document.querySelector(".inbox-project-head");
    await vi.waitFor(() => expect(head()).not.toBe(null), WAIT);
    await vi.waitFor(() => expect(rowFor(mine.id)).not.toBe(null), WAIT);
    expect(rowFor(quietTask.id)).toBe(null);
    expect(head().querySelector(".inbox-status-dot")).toBe(null);
    head().querySelector("[data-project-fold]").click();
    await vi.waitFor(() => expect(head().querySelector(".inbox-status-running")).not.toBe(null), WAIT);
    await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([{ ...quietTask, watched: false }, mine], []));
    await vi.waitFor(() => expect(head().querySelector(".inbox-status-dot")).toBe(null), WAIT);
    await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([quietTask, mine], []));
    await vi.waitFor(() => expect(head().querySelector(".inbox-status-running")).not.toBe(null), WAIT);
    await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([{ ...quietTask, status: "done" }, mine], []));
    await vi.waitFor(() => expect(head().querySelector(".inbox-status-dot")).toBe(null), WAIT);
    expect(call).not.toHaveBeenCalled();
  });

  it("resolves legacy project-agent tasks from the project's owner roster, including hidden task work", async () => {
    const legacy = { ...assigned, assignee: { kind: "project_agent" } };
    const row = (agents) => ({ ...agentRow(agents), run_id: "run-project" });
    const land = (agents) => modules.cache.writeCached({ deviceId: DEVICE, entityId: "run-project", kind: "row" }, row(agents));
    const activePrimary = [{ id: "agent-task", working: false, surfaces: { subagents: [{ state: "running" }] } }];
    await boot({ greet: false, tasks: [legacy], projectEntityId: "run-project", agentRows: [row(activePrimary)] });
    await vi.waitFor(() => expect(dotFor()?.classList.contains("inbox-status-running")).toBe(true), WAIT);

    // An idle project holder does not borrow unrelated secondary work.
    await land([{ id: "agent-primary", working: false }, ...activePrimary]);
    await vi.waitFor(() => expect(dotFor()).toBe(null), WAIT);
    // The holder rule prefers the project's own working agent when one is known.
    await land([{ id: "agent-primary", working: false }, { id: "agent-task", working: true }]);
    await vi.waitFor(() => expect(dotFor()?.classList.contains("inbox-status-running")).toBe(true), WAIT);

    await land(activePrimary);
    const mine = task({ id: "task-mine", watched: true, assignee: { kind: "user" } });
    await modules.needsYouRule.rememberNeedsYouRule(DEVICE, { tasks: { commentUserNotifies: true } });
    await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([
      { ...legacy, status: "in_progress" }, mine,
    ], []));
    modules.inboxView.setInboxView("projects");
    await vi.waitFor(() => expect(rowFor(mine.id)).not.toBe(null), WAIT);
    const head = () => document.querySelector(".inbox-project-head");
    head().querySelector("[data-project-fold]").click();
    await vi.waitFor(() => expect(head().querySelector(".inbox-status-running")).not.toBe(null), WAIT);
    expect(rowFor(legacy.id)).toBe(null);
    expect(call).not.toHaveBeenCalled();
  });
});

const unwatch = async () => {
  rowFor(review.id).querySelector("[data-menu]").click();
  await vi.waitFor(() => expect(rowFor(review.id).querySelector("[data-unwatch]")).not.toBe(null), WAIT);
  rowFor(review.id).querySelector("[data-unwatch]").click();
};

describe("a watched task's inbox row", () => {
  beforeEach(() => boot());

  it("paints from the cache on a reload, on both faces, and opens the task", async () => {
    await vi.waitFor(() => expect(rowFor(review.id)?.querySelector(".inbox-facts")?.textContent).toBe("In review"), WAIT);
    expect(call.mock.calls.some(([method]) => method === "tasks.list" || method === "tasks.get")).toBe(false);
    modules.inboxView.setInboxView("projects");
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    expect(rowFor(review.id).closest(".inbox-project")?.dataset.project).toBe(project.projectKey);
    modules.inboxView.setInboxView("inbox");
  });

  it("goes the moment Stop watching is pressed, and the cached list says so", async () => {
    answers["tasks.unwatch"] = async () => ({ task: { ...review, watched: false } });
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    await unwatch();
    await vi.waitFor(() => expect(rowFor(review.id)).toBe(null), WAIT);
    // #286: the row goes on the cache write, and the unwatch is sent only after
    // that write settles, so the call can come a few IndexedDB turns later.
    await vi.waitFor(() => expect(call).toHaveBeenCalledWith("tasks.unwatch", { task_id: review.id }), WAIT);
    const held = await modules.tracker.readTasksRecord(DEVICE, PROJECT);
    expect(held.tasks[0].watched).toBe(false);
  });

  it("stays gone when a list asked before the unwatch landed arrives after the click", async () => {
    let answer;
    answers["tasks.unwatch"] = () => new Promise((resolve) => { answer = resolve; });
    // A second watched task in review is how each list is seen to have been
    // painted: its row, and its title, come from that list and nothing else.
    const other = task({ id: "task-8", number: 8, title: "Marker", watched: true, status: "in_review",
      updated_at: "2026-09-24T01:01:00Z" });
    // What the sync layer writes when an `tasks.list` answer lands
    // (core/cacheSync.js readTasks): the whole list, over whatever was held.
    const land = (tasks) => modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord(tasks, []));
    const titleOf = (id) => rowFor(id)?.querySelector(".stitle")?.textContent;
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    await unwatch();
    await vi.waitFor(() => expect(rowFor(review.id)).toBe(null), WAIT);

    // While the unwatch is in flight: a list from before it, still watched.
    await land([review, other]);
    await vi.waitFor(() => expect(titleOf(other.id)).toBe("#8 Marker"), WAIT);
    expect(rowFor(review.id)).toBe(null);

    // Answered at 01:05; a list the sync layer asked for before that lands.
    await vi.waitFor(() => expect(answer).toBeTypeOf("function"), WAIT);
    answer({ task: { ...review, watched: false, updated_at: "2026-09-24T01:05:00Z" } });
    await land([{ ...review, updated_at: "2026-09-24T01:04:00Z" }, { ...other, title: "Marker again" }]);
    await vi.waitFor(() => expect(titleOf(other.id)).toBe("#8 Marker again"), WAIT);
    expect(rowFor(review.id)).toBe(null);

    // Watched again after the unwatch, on the task page: the row is back.
    await land([{ ...review, updated_at: "2026-09-24T01:10:00Z" }, other]);
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
  });

  it("comes back if the bridge refuses the unwatch", async () => {
    let refuse;
    answers["tasks.unwatch"] = () => new Promise((_, reject) => { refuse = reject; });
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    await unwatch();
    await vi.waitFor(() => expect(rowFor(review.id)).toBe(null), WAIT);
    await vi.waitFor(() => expect(refuse).toBeTypeOf("function"), WAIT);
    refuse(new Error("Build cannot stop watching that task."));
    await vi.waitFor(() => expect(rowFor(review.id)).not.toBe(null), WAIT);
    const held = await modules.tracker.readTasksRecord(DEVICE, PROJECT);
    expect(held.tasks[0].watched).toBe(true);
  });
});

describe("a watched task's inbox row before any machine answers (#144)", () => {
  const mine = task({ id: "task-9", number: 9, title: "Pick the fix", watched: true, status: "in_progress",
    assignee: { kind: "user" }, updated_at: "2026-09-24T01:00:00Z" });

  beforeEach(() => boot({ greet: false, tasks: [review, mine], rule: { tasks: { commentUserNotifies: true } } }));

  it("paints what the cache says needs the user, by the cached rule, with no greeting", async () => {
    await vi.waitFor(() => expect(rowFor(mine.id)?.querySelector(".inbox-facts")?.textContent).toBe("Assigned to you"), WAIT);
    // In review between agents is not the user's business by the cached rule.
    expect(rowFor(review.id)).toBe(null);
    expect(call).not.toHaveBeenCalled();
    expect(modules.deviceContexts.contextFor(DEVICE)).toBe(null);
  });
});

describe("a mentioned creation on a cold reload", () => {
  const asked = task({ id: "task-created-ask", number: 11, title: "Choose the route", watched: true,
    status: "backlog", updated_at: "2026-09-24T01:00:00Z" });
  const created = { type: "event", id: "te-01K5Z3", task_id: asked.id, kind: "created",
    actor: { kind: "agent", agent_id: "agent-astra" }, mentions_user: true,
    payload: { title: asked.title }, at: "2026-09-24T01:00:00Z" };
  let pane;

  beforeEach(() => boot({ greet: false, tasks: [asked], timelines: { [asked.id]: [created] },
    rule: { tasks: { commentUserNotifies: true } } }));
  afterEach(() => pane?.dispose());

  it("paints Needs you and the inbox from cache, then drops both when the read mark passes creation", async () => {
    const host = document.body.appendChild(document.createElement("div"));
    pane = modules.tasksPane.mountTasksPane(host, {
      projectId: PROJECT, projectName: "Build", deviceId: DEVICE, projectKey: project.projectKey,
      callRpc: call, catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
      feed: () => ({ workspaces: [], items: [], projects: [] }), defaultView: "dashboard", navigate: () => {},
    });
    const needsYou = () => [...host.querySelectorAll('[data-dashboard-section="needsYou"] .task-dashboard-row')]
      .map((row) => row.dataset.task);
    await vi.waitFor(() => expect(rowFor(asked.id)?.querySelector(".inbox-facts")?.textContent).toBe("Mentioned you"), WAIT);
    await vi.waitFor(() => expect(needsYou()).toEqual([asked.id]), WAIT);
    expect(modules.deviceContexts.contextFor(DEVICE)).toBe(null);

    const read = { ...asked, read_through: created.id, updated_at: "2026-09-24T01:01:00Z" };
    await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([read], []));
    await modules.tracker.writeTaskRecord(DEVICE, PROJECT, read.id, taskDetail(read, [created]));
    await vi.waitFor(() => expect(rowFor(asked.id)).toBe(null), WAIT);
    await vi.waitFor(() => expect(needsYou()).toEqual([]), WAIT);
    expect(modules.deviceContexts.contextFor(DEVICE)).toBe(null);
  });
});

describe("Stop watching a task an agent asked the user about (#144)", () => {
  // Agent work, so only the unread question put it in Needs you, and the
  // board's feed row for it is from before the question: it counts nothing.
  const asked = task({ id: "task-10", number: 10, title: "Which fix", watched: true, status: "in_progress",
    assignee: { kind: "agent", agent_id: "agent-astra" }, read_through: "te-01K5Z1",
    updated_at: "2026-09-24T01:00:00Z" });
  const question = comment({ id: "tc-01K5Z3", task_id: asked.id, author: { kind: "agent", agent_id: "agent-astra" },
    body: "Which of the two fixes do you want?", notifies_user: true });
  const staleRow = { kind: "tracker_task", projectKey: project.projectKey, task_id: asked.id, unread: 0 };
  let pane;

  beforeEach(() => boot({
    tasks: [asked],
    timelines: { [asked.id]: [question] },
    hello: { api_version: "2.0.0", push_events: true, capabilities: ["tasks.watching", "tasks.commentUserNotifies"] },
  }));
  afterEach(() => pane?.dispose());

  /** The Dashboard for this project, mounted beside the inbox, and the
   *  tasks in its Needs you. */
  const mountDashboard = () => {
    const host = document.body.appendChild(document.createElement("div"));
    pane = modules.tasksPane.mountTasksPane(host, {
      projectId: PROJECT, projectName: "Build", deviceId: DEVICE, projectKey: project.projectKey,
      callRpc: call, catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
      feed: () => ({ workspaces: [], items: [staleRow], projects: [] }), defaultView: "dashboard", navigate: () => {},
    });
    return () => [...host.querySelectorAll('[data-dashboard-section="needsYou"] .task-dashboard-row')]
      .map((row) => row.dataset.task);
  };
  const pressStopWatching = async () => {
    rowFor(asked.id).querySelector("[data-menu]").click();
    await vi.waitFor(() => expect(rowFor(asked.id).querySelector("[data-unwatch]")).not.toBe(null), WAIT);
    rowFor(asked.id).querySelector("[data-unwatch]").click();
  };
  const heldList = async () => (await modules.cache.readCached(modules.tracker.tasksAddress(DEVICE, PROJECT))).value;

  it("leaves the Dashboard when it leaves the inbox, whatever the board's row still says", async () => {
    await vi.waitFor(async () => expect(await modules.needsYouRule.readNeedsYouRule(DEVICE)).toBe(true), WAIT);
    const needsYou = mountDashboard();
    await vi.waitFor(() => expect(rowFor(asked.id)).not.toBe(null), WAIT);
    await vi.waitFor(() => expect(needsYou()).toEqual([asked.id]), WAIT);

    answers["tasks.unwatch"] = async () => ({ task: { ...asked, watched: false } });
    await pressStopWatching();
    await vi.waitFor(() => expect(rowFor(asked.id)).toBe(null), WAIT);
    expect((await modules.tracker.readTasksRecord(DEVICE, PROJECT)).tasks[0].watched).toBe(false);
    await vi.waitFor(() => expect(needsYou()).toEqual([]), WAIT);
  });

  // #129: Stop watching is a list written here, so it is newer than every read
  // asked before it — including the Dashboard's own answer, asked after the
  // whole list was, which the tab would otherwise keep over it.
  describe("with the Dashboard's own answer newer than the whole list", () => {
    beforeEach(async () => {
      const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
      const { DEFAULT_FILTERS, taskListParams } = await import("../src/core/trackerFilters.js");
      const dashboardParams = taskListParams(PROJECT, { ...DEFAULT_FILTERS, state: "" });
      await modules.tracker.writeTasksRecord(DEVICE, PROJECT, modules.tracker.tasksRecord([asked], [], await nextTaskRead()));
      await modules.tracker.writeTasksQueryRecord(DEVICE, PROJECT, dashboardParams,
        modules.tracker.tasksRecord([asked], [], await nextTaskRead()));
      await vi.waitFor(async () => expect(await modules.needsYouRule.readNeedsYouRule(DEVICE)).toBe(true), WAIT);
    });

    it("leaves the Dashboard's Needs you on Stop watching", async () => {
      const needsYou = mountDashboard();
      await vi.waitFor(() => expect(needsYou()).toEqual([asked.id]), WAIT);

      answers["tasks.unwatch"] = async () => ({ task: { ...asked, watched: false } });
      await pressStopWatching();
      await vi.waitFor(() => expect(rowFor(asked.id)).toBe(null), WAIT);
      expect((await heldList()).tasks[0].watched).toBe(false);
      await vi.waitFor(() => expect(needsYou()).toEqual([]), WAIT);
    });

    it("comes back to the Dashboard's Needs you when the bridge refuses, numbered after the unwatch", async () => {
      const needsYou = mountDashboard();
      await vi.waitFor(() => expect(needsYou()).toEqual([asked.id]), WAIT);

      let refuse;
      answers["tasks.unwatch"] = () => new Promise((_, reject) => { refuse = reject; });
      await pressStopWatching();
      await vi.waitFor(() => expect(needsYou()).toEqual([]), WAIT);
      const unwatchedAs = (await heldList()).read_order;

      await vi.waitFor(() => expect(refuse).toBeTypeOf("function"), WAIT);
      refuse(new Error("Build cannot stop watching that task."));
      await vi.waitFor(() => expect(rowFor(asked.id)).not.toBe(null), WAIT);
      const restored = await heldList();
      expect(restored.tasks[0].watched).toBe(true);
      expect(restored.read_order).toBeGreaterThan(unwatchedAs);
      await vi.waitFor(() => expect(needsYou()).toEqual([asked.id]), WAIT);
    });
  });
});
