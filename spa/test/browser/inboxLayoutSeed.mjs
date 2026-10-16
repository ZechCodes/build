import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

export const LAYOUT_DEVICE = "inbox-layout-device";
export const LAYOUT_NOW = new Date("2026-10-16T12:00:00Z");
export const LAYOUT_ROWS = [
  { key: `workspace:${LAYOUT_DEVICE}/live-workspace`, project: "Build", name: "Inbox layout", recent: false },
  { key: `project-agent:${LAYOUT_DEVICE}/live-project`, project: "Build", name: "Build", recent: false },
  { key: "tracker_task:live-layout-task", project: "Build", name: "#629 Make the inbox easier to read", recent: false },
  { key: `workspace:${LAYOUT_DEVICE}/quiet-workspace`, project: "Notes", name: "Saved research", recent: true },
  { key: `project-agent:${LAYOUT_DEVICE}/quiet-project`, project: "Notes", name: "Notes", recent: true },
  { key: "tracker_task:quiet-layout-task", project: "Notes", name: "#628 Keep the current ordering", recent: true },
];
const body = readFileSync(fileURLToPath(new URL("../../index.html", import.meta.url)), "utf8")
  .match(/<body>([\s\S]*)<\/body>/)[1];

/** A mounted production rail fed entirely by local records. Its fake device
 * session answers no transport, account or bridge and records only row opens. */
export async function mountInboxLayout(page, basePath) {
  await page.clock.setFixedTime(LAYOUT_NOW);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mountLayout(page, body, { basePath });
  await loadBrowserModules(page, {
    app: "src/appState.js", cache: "src/core/localCache.js", tracker: "src/core/trackerCache.js",
    feed: "src/core/taskFeed.js", rail: "src/core/inboxShell.js", inbox: "src/core/inboxView.js",
    contexts: "src/core/deviceContexts.js",
  }, basePath);
  await page.evaluate(async (deviceId) => {
    const { app, cache, tracker, feed, rail, inbox, contexts } = window.__layoutModules;
    Object.assign(app.App, { route: { name: "inbox" }, devices: [{ id: deviceId, name: "This computer" }],
      selectedDeviceId: deviceId, deviceFilter: null });
    const now = Date.now();
    const minutesAgo = (minutes) => now - minutes * 60_000;
    const projects = [
      { id: "live-project", name: "Build", entity_id: "live-project-run" },
      { id: "quiet-project", name: "Notes", entity_id: "quiet-project-run" },
    ].map((project) => ({ ...project, project_id: project.id, projectKey: `${deviceId}/${project.id}`, deviceId }));
    const workspace = (id, project, name, age) => ({ id, name, project_id: project.id,
      deviceId, projectKey: project.projectKey, workspaceKey: `${deviceId}/${id}`, entity_id: `${id}-run`,
      status: "ready", managed: true, can_finish: false, finish_blockers: ["dirty"],
      directories: [{ id: "source", source_id: "source", is_git: true }],
      session_started_ms: minutesAgo(age + 30), last_activity_ms: minutesAgo(age),
      work_summary: { pushes: 1, behind: 0, additions: 18, deletions: 3 } });
    const workspaces = [workspace("live-workspace", projects[0], "Inbox layout", 3),
      workspace("quiet-workspace", projects[1], "Saved research", 2000)];
    const conversation = (project, runId, age, unread = 0) => ({ kind: "branch", project_id: project.id,
      projectKey: project.projectKey, deviceId, run_id: runId,
      session_started_ms: minutesAgo(age + 20), last_activity_ms: minutesAgo(age),
      agents: [{ id: `${runId}-agent`, watched: true, working: false, unread_count: unread }] });
    const items = [conversation(projects[0], projects[0].entity_id, 2, 2),
      conversation(projects[1], projects[1].entity_id, 2000, 1),
      ...workspaces.map((row, index) => conversation(projects[index], row.entity_id, index ? 2000 : 3, 1))];
    const task = (id, number, title, project, age) => ({ id, number, title, project_id: project.id,
      state: "open", status: "in_progress", watched: true, assignee: { kind: "user" },
      unread_count: 0, created_at: new Date(minutesAgo(age + 10)).toISOString(),
      updated_at: new Date(minutesAgo(age)).toISOString() });
    const tasks = [task("live-layout-task", 629, "Make the inbox easier to read", projects[0], 1),
      task("quiet-layout-task", 628, "Keep the current ordering", projects[1], 2000)];
    const address = (kind) => ({ deviceId, entityId: "", kind });
    await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
    await cache.writeCached(address("projects"), projects);
    await cache.writeCached(address("workspaces"), workspaces);
    await cache.writeCached(address("feed"), { items, runs: items, projects, workspaces });
    for (const project of projects) {
      const own = tasks.filter((row) => row.project_id === project.id);
      await tracker.writeTasksRecord(deviceId, project.id, tracker.tasksRecord(own, []));
      for (const task of own) await tracker.writeTaskRecord(deviceId, project.id, task.id, { task, timeline: [] });
    }
    window.__inboxLayoutCalls = [];
    contexts.adoptDeviceSession({ deviceId, call: async (method, params) => {
      window.__inboxLayoutCalls.push({ method, params });
      return {};
    }, close() {}, peer() {}, onCarrier() {}, onPush() {} });
    await rail.initInboxRail();
    inbox.setInboxView("inbox");
    await feed.startFeed();
    if (innerWidth <= 900) await rail.setInboxCollapsed(true, { animate: false, persist: false, reveal: true });
  }, LAYOUT_DEVICE);
  await page.locator('[data-key="tracker_task:live-layout-task"]').waitFor({ state: "visible" });
}

/** Open each face's existing Recent disclosures without relying on row counts
 * that intentionally changed when project agents moved into their blocks. */
export async function showInboxLayoutFace(page, face) {
  await page.evaluate((face) => window.__layoutModules.inbox.setInboxView(face), face);
  for (const toggle of await page.locator('#inbox-list [data-recent-toggle][aria-expanded="false"]').all()) {
    const scope = await toggle.getAttribute("data-recent-toggle");
    await toggle.click();
    await page.waitForFunction((scope) => [...document.querySelectorAll("[data-recent-toggle]")]
      .some((node) => node.dataset.recentToggle === scope && node.getAttribute("aria-expanded") === "true"), scope);
  }
  await page.evaluate(() => document.activeElement?.blur());
  await page.mouse.move(0, 0);
  await page.evaluate(() => document.fonts.ready);
}

export async function disposeInboxLayout(page) {
  await page.evaluate(() => {
    const { inbox, feed, contexts } = window.__layoutModules;
    inbox.unmountInboxList();
    feed.stopFeed();
    contexts.resetDeviceContexts();
  });
}
