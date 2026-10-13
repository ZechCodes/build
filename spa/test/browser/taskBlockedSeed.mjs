import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";
export const BLOCK_DEVICE = "blocked-device";
export const BLOCK_PROJECT = "blocked-project";
export const HOSTILE_TITLE = '<img src=x onerror="window.__injected=true"> & blocked work';
const shell = readFileSync(fileURLToPath(new URL("../../index.html", import.meta.url)), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
export function blockedTasks({ legacy = false } = {}) {
  const now = new Date().toISOString();
  const make = (id, number, title, over = {}) => ({ id, number, title, project_id: BLOCK_PROJECT,
    state: "open", status: "backlog", watched: true, body: "", priority: "medium", assignee: null,
    trackers: [], labels: [], read_through: null, links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
    dependencies: { blocked: false, waiting_on: [] }, created_by: { kind: "user" }, created_at: now, updated_at: now, ...over });
  const blocked = { blocked: true, waiting_on: [
    { task_id: "missing-blocker", number: 558, title: '<script>window.__injected=true</script>', resolved: false },
    { task_id: "resolved-blocker", number: 562, title: "Merged prerequisite", resolved: true }] };
  const held = { kind: "agent", agent_id: "busy-agent" };
  const tasks = [
    make("working", 590, "Runnable agent work", { status: "in_progress", assignee: held }),
    make("assigned", 589, "Queued for the idle agent", { status: "ready", assignee: { kind: "agent", agent_id: "idle-agent" } }),
    make("blocked-running", 588, HOSTILE_TITLE, { priority: "urgent", assignee: held, dependencies: blocked }),
    make("blocked-user", 587, "User assignment waits for prerequisites", { assignee: { kind: "user" }, dependencies: blocked }),
    make("blocked-missing", 586, "Missing holder still waits", { assignee: { kind: "agent", agent_id: "missing-agent" }, dependencies: blocked }),
    make("blocked-new-workspace", 585, "A workspace will be created when ready", { dependencies: blocked,
      deferred_assignment: { state: "waiting", target: { kind: "new_workspace", name: "Implementation" } } }),
    make("backlog-blocked-high", 584, "Blocked urgent backlog", { priority: "urgent", dependencies: blocked }),
    make("backlog-blocked-low", 583, "Blocked ordinary backlog", { dependencies: blocked }),
    make("backlog-ready-high", 582, "Ready urgent backlog", { priority: "urgent" }),
    make("backlog-ready-low", 581, "Ready ordinary backlog"),
    make("done", 580, "Done keeps its history", { assignee: held, status: "done", dependencies: blocked }),
    make("closed", 579, "Closed keeps its history", { assignee: held, state: "closed", dependencies: blocked }),
    make("review", 578, "Review keeps its history", { assignee: held, status: "in_review", dependencies: blocked }),
  ];
  if (legacy) for (const task of tasks) { delete task.dependencies; delete task.deferred_assignment; }
  return tasks;
}

/** Mock wire is held open while production cache observers mount. */
export async function mountBlockedTasks(page, basePath, { legacy = false, theme = "dark" } = {}) {
  await mountLayout(page, '<main id="blocked-tasks"></main><aside id="blocked-agent-rail" aria-label="Agent tasks"><h2>Agent tasks</h2><div id="blocked-rail-viewer"></div></aside>', {
    basePath, styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block;padding:16px} #blocked-tasks{max-width:900px;margin:auto} #blocked-agent-rail{max-width:900px;margin:24px auto}.surface-rows{display:grid;gap:8px}`,
  });
  await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
  await loadBrowserModules(page, { cache: "src/core/localCache.js", tracker: "src/core/trackerCache.js", tasksPane: "src/core/trackerTasksPane.js",
    changes: "src/core/changeEvents.js", railTasks: "src/core/trackerAgentTasksEntry.js", surfaces: "src/core/agentSurfaces.js", rule: "src/core/needsYouRule.js" }, basePath);
  await page.evaluate(async ({ tasks, deviceId, projectId }) => {
    const { cache, tracker, tasksPane, changes, railTasks, surfaces, rule } = window.__layoutModules;
    const projectKey = `${deviceId}/${projectId}`;
    const owner = { run_id: "blocked-run", project_id: projectId, projectKey, deviceId,
      agents: [{ id: "busy-agent", name: "Implementer", working: true, watched: true }, { id: "idle-agent", name: "Reviewer", working: false, watched: true }] };
    const feed = { projects: [{ id: projectId, project_id: projectId, projectKey, name: "Build" }],
      workspaces: [{ id: "blocked-workspace", projectKey, name: "Implementation", entity_id: owner.run_id }], items: [owner] };
    window.__blockedTasks = tasks;
    window.__blockedCalls = [];
    window.__blockedWireOpen = false;
    const pending = [];
    const reply = (method, params) => {
      if (method === "session.hello") return { api_version: "3.21.0", push_events: true,
        capabilities: ["tasks.waitingOn", "tasks.commentUserNotifies", "changes.subscriptions"], change_kinds: ["tasks"] };
      if (method === "tasks.list") return { tasks: window.__blockedTasks };
      if (method === "tasks.columns") return { columns: [] };
      if (method === "tasks.get") return { task: window.__blockedTasks.find((task) => task.id === params.task_id), timeline: [] };
      return {};
    };
    const call = async (method, params) => {
      window.__blockedCalls.push({ method, params });
      if (!window.__blockedWireOpen) return new Promise((resolve) => pending.push(() => resolve(reply(method, params))));
      return reply(method, params);
    };
    await cache.writeCached({ deviceId, entityId: owner.run_id, kind: "row" }, owner);
    await tracker.writeTasksRecord(deviceId, projectId, tracker.tasksRecord(tasks, []));
    for (const task of tasks) await tracker.writeTaskRecord(deviceId, projectId, task.id, tracker.taskRecord(task, []));
    await rule.rememberNeedsYouRule(deviceId, { tasks: { commentUserNotifies: true } });
    const viewer = surfaces.mountSurfaceViewer(document.querySelector("#blocked-rail-viewer"), "tasks", {});
    addEventListener("pagehide", () => viewer.dispose(), { once: true });
    const paintRail = () => {
      if (window.__blockedRail) viewer.set({ tasks: window.__blockedRail.entriesFor("busy-agent", true) || [] });
    };
    window.__blockedRail = railTasks.mountAgentTasks({ deviceId, projectId, onChanged: paintRail });
    window.__blockedPane = tasksPane.mountTasksPane(document.querySelector("#blocked-tasks"), { deviceId, projectId, projectName: "Build", projectKey,
      defaultView: "dashboard", feed: () => feed, callRpc: call, catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {} });
    window.__blockedGreet = async () => {
      window.__blockedWireOpen = true;
      for (const resolve of pending.splice(0)) resolve();
      await changes.greetBridge(call, { deviceId });
    };
  }, { tasks: blockedTasks({ legacy }), deviceId: BLOCK_DEVICE, projectId: BLOCK_PROJECT });
  await page.locator('[data-dashboard-tab="active"]').waitFor();
  await page.locator('#blocked-agent-rail .surface-task').first().waitFor();
}
export async function pushBlockedTask(page, taskId, patch) {
  await page.evaluate(async ({ taskId, patch, deviceId, projectId }) => {
    const { changes, tracker } = window.__layoutModules;
    await window.__blockedGreet();
    window.__blockedTasks = window.__blockedTasks.map((task) => task.id === taskId ? { ...task, ...patch, updated_at: new Date(Date.now() + 5000).toISOString() } : task);
    const task = window.__blockedTasks.find((one) => one.id === taskId);
    await tracker.writeTaskRecord(deviceId, projectId, taskId, tracker.taskRecord(task, []));
    changes.dispatchChangeEvent({ type: "changes", items: [{ entity_id: projectId, tasks: { task_ids: [taskId], truncated: false } }] }, deviceId);
  }, { taskId, patch, deviceId: BLOCK_DEVICE, projectId: BLOCK_PROJECT });
}
export async function mountBlockedInbox(page, basePath, { question = false, theme = "dark" } = {}) {
  await mountLayout(page, shell, { basePath });
  await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
  await loadBrowserModules(page, { app: "src/appState.js", cache: "src/core/localCache.js", tracker: "src/core/trackerCache.js",
    feed: "src/core/taskFeed.js", rail: "src/core/inboxShell.js", inbox: "src/core/inboxView.js", rule: "src/core/needsYouRule.js" }, basePath);
  await page.evaluate(async ({ tasks, deviceId, projectId, question }) => {
    const { app, cache, tracker, feed, rail, inbox, rule } = window.__layoutModules;
    Object.assign(app.App, { route: { name: "inbox" }, devices: [{ id: deviceId, name: "This computer" }], selectedDeviceId: deviceId, deviceFilter: null });
    const project = { id: projectId, project_id: projectId, name: "Build", deviceId, projectKey: `${deviceId}/${projectId}`, entity_id: "blocked-run" };
    const owner = { run_id: project.entity_id, project_id: projectId, projectKey: project.projectKey, deviceId, kind: "branch",
      agents: [{ id: "busy-agent", watched: true, working: true, unread_count: 0 }] };
    await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
    for (const [kind, value] of [["projects", [project]], ["workspaces", []], ["feed", { projects: [project], workspaces: [], items: [owner], runs: [owner] }]])
      await cache.writeCached({ deviceId, entityId: "", kind }, value);
    await tracker.writeTasksRecord(deviceId, projectId, tracker.tasksRecord(tasks, []));
    for (const task of tasks) {
      const timeline = question && task.id === "blocked-user" ? [{ type: "comment", id: "blocked-question", task_id: task.id,
        author: { kind: "agent", id: "busy-agent" }, body: "Which release should this target?", refs: [], mentions_user: true,
        notifies_user: true, created_at: new Date().toISOString() }] : [];
      await tracker.writeTaskRecord(deviceId, projectId, task.id, tracker.taskRecord(task, timeline));
    }
    await rule.rememberNeedsYouRule(deviceId, { tasks: { commentUserNotifies: true } });
    await rail.initInboxRail(); inbox.setInboxView("inbox"); await feed.startFeed();
    if (innerWidth <= 900) await rail.setInboxCollapsed(true, { animate: false, persist: false, reveal: true });
  }, { tasks: blockedTasks(), deviceId: BLOCK_DEVICE, projectId: BLOCK_PROJECT, question });
  await page.locator('[data-key="tracker_task:blocked-user"]').waitFor();
}
