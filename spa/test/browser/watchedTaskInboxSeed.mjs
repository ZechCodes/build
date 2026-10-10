import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

export const WATCH_DEVICE = "watched-task-device";
export const WATCH_PROJECT = "watched-task-project";
export const QUIET_TASK = "quiet-watched-task";
export const ASK_TASK = "asked-watched-task";
const shell = readFileSync(fileURLToPath(new URL("../../index.html", import.meta.url)), "utf8")
  .match(/<body>([\s\S]*)<\/body>/)[1];

/** Cold cached inbox, mounted through its production feed and watch follower.
 * Painting has no session; only the action tests subsequently adopt a fake one. */
export async function mountWatchedTaskInbox(page, basePath, { unread = false } = {}) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mountLayout(page, shell, { basePath });
  await loadBrowserModules(page, {
    app: "src/appState.js", cache: "src/core/localCache.js", tracker: "src/core/trackerCache.js",
    feed: "src/core/taskFeed.js", rail: "src/core/inboxShell.js", inbox: "src/core/inboxView.js",
    rule: "src/core/needsYouRule.js", contexts: "src/core/deviceContexts.js",
  }, basePath);
  await page.evaluate(async ({ deviceId, projectId, quietId, askId, unread }) => {
    const { app, cache, tracker, feed, rail, inbox, rule } = window.__layoutModules;
    Object.assign(app.App, { route: { name: "inbox" }, devices: [{ id: deviceId, name: "This computer" }],
      selectedDeviceId: deviceId, deviceFilter: null });
    const project = { id: projectId, project_id: projectId, name: "Build", deviceId, projectKey: `${deviceId}/${projectId}`,
      entity_id: "watched-project-run" };
    const makeTask = (id, number, title, over = {}) => ({
      id, number, title, project_id: projectId, state: "open", status: "in_progress", watched: true,
      body: "", labels: [], priority: "none", assignee: null, trackers: [], read_through: null,
      links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
      created_by: { kind: "user" }, created_at: "2026-10-10T10:00:00Z", updated_at: "2026-10-10T10:00:00Z",
      ...over,
    });
    const tasks = [
      makeTask(quietId, 475, "Quiet task"),
      makeTask(askId, 476, "Your next choice", { assignee: { kind: "user" },
        created_at: "2026-10-10T10:03:00Z", updated_at: "2026-10-10T10:03:00Z" }),
      makeTask("review-watched-task", 477, "Agent review", { status: "in_review",
        created_at: "2026-10-10T10:06:00Z", updated_at: "2026-10-10T10:06:00Z" }),
      makeTask("done-watched-task", 478, "Completed inbox work", { status: "done" }),
      makeTask("closed-watched-task", 479, "Closed inbox work", { state: "closed" }),
      makeTask("unwatched-task", 480, "Unwatched inbox work", { watched: false }),
    ];
    await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
    const address = (kind) => ({ deviceId, entityId: "", kind });
    await cache.writeCached(address("projects"), [project]);
    await cache.writeCached(address("workspaces"), []);
    const owner = { kind: "branch", deviceId, project_id: projectId, projectKey: project.projectKey,
      run_id: project.entity_id, agents: [{ id: "watched-project-agent", watched: true, working: false, unread_count: 0 }] };
    await cache.writeCached(address("feed"), { items: [owner], runs: [owner], projects: [project], workspaces: [] });
    await tracker.writeTasksRecord(deviceId, projectId, tracker.tasksRecord(tasks, []));
    for (const task of tasks) {
      const timeline = unread && task.id === quietId ? [{ type: "comment", id: "tc-0001", task_id: quietId,
        author: { kind: "agent", id: "watched-project-agent" }, body: "A task update", refs: [],
        mentions_user: false, notifies_user: false, created_at: "2026-10-10T10:00:00Z" }] : [];
      await tracker.writeTaskRecord(deviceId, projectId, task.id, tracker.taskRecord(task, timeline));
    }
    await rule.rememberNeedsYouRule(deviceId, { tasks: { commentUserNotifies: true } });
    await rail.initInboxRail();
    inbox.setInboxView("inbox");
    await feed.startFeed();
    if (innerWidth <= 900) await rail.setInboxCollapsed(true, { animate: false, persist: false, reveal: true });
    window.__watchedTaskFixture = tasks;
  }, { deviceId: WATCH_DEVICE, projectId: WATCH_PROJECT, quietId: QUIET_TASK, askId: ASK_TASK, unread });
  await page.locator(`[data-key="tracker_task:${ASK_TASK}"]`).waitFor({ state: "visible" });
  await page.evaluate(() => document.fonts.ready);
}

/** Keep unwatch pending to inspect the optimistic cache write, then optionally
 * refuse it to exercise restoration. No bridge, network or account is involved. */
export async function adoptWatchedTaskSession(page) {
  await page.evaluate((deviceId) => {
    window.__watchedTaskCalls = [];
    const call = (method, params) => {
      window.__watchedTaskCalls.push({ method, params });
      if (method === "tasks.unwatch") return new Promise((resolve, reject) => {
        window.__settleWatchedTaskUnwatch = { resolve, reject };
      });
      return new Promise(() => {});
    };
    window.__layoutModules.contexts.adoptDeviceSession({ deviceId, call,
      close() {}, peer() {}, onCarrier() {}, onPush() {} });
  }, WATCH_DEVICE);
}

export async function disposeWatchedTaskInbox(page) {
  await page.evaluate(() => {
    const { inbox, feed, contexts } = window.__layoutModules;
    inbox.unmountInboxList();
    feed.stopFeed();
    contexts.resetDeviceContexts();
  });
}
