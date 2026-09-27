// Capture the review images for #190 in a real Chromium against the production
// app: every place a task wears its mark, now a checkbox where GitHub's circle
// was — the workspace rail's Tasks face, the board's cards (empty, checked
// and slashed, and a closed task in Done: checked, in the closed colour), and
// the head of a task's own page for each of the four — in
// the dark theme and the light one. The inbox and the dashboard's Needs you are
// captured beside them: their rows wear the row's unread dot, never a task mark.
// One scripted machine answers the tracker; everything else is the app's own.
// Run from spa/: node test/browser/captureTaskCheckbox.mjs [output directory]
import { mkdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

const output = resolve(process.argv[2] || "../design/task-checkbox");
await mkdir(output, { recursive: true });
const indexHtml = await readFile(new URL("../../index.html", import.meta.url), "utf8");
const bodyHtml = indexHtml.match(/<body>([\s\S]*)<\/body>/)[1];
const DEVICE = "dev-1";

/** Runs in the page: the machine's records and a session answering them,
 *  then `stand` puts the app where the capture wants it. */
async function seed({ device, hash, inbox }) {
  const { app, cache, feed, merge, contexts, toolbar, events, router, trackerCache, shell } = window.__layoutModules;
  const now = Date.now();
  const minutes = (count) => now - count * 60_000;
  const iso = (ms) => new Date(ms).toISOString();
  const project = { project_id: "p-1", id: "p-1", name: "Build", entity_id: "project-run",
    session_started_ms: minutes(90), last_activity_ms: minutes(2) };
  const workspace = { id: "ws-1", workspace_id: "ws-1", project_id: "p-1", name: "Checkout flow", status: "ready",
    managed: true, can_finish: true, entity_id: "run-1", session_started_ms: minutes(80), last_activity_ms: minutes(3),
    directories: [{ source_id: "build", name: "Build", is_git: true }],
    work_summary: { pushes: 2, behind: 0, additions: 184, deletions: 37 } };
  const workspaceRun = { kind: "branch", run_id: "run-1", project_id: "p-1", unread: true, working: true,
    agents: [{ id: "builder", name: "Builder", watched: true, working: true, unread_count: 0 }] };
  const projectRun = { kind: "branch", run_id: "project-run", project_id: "p-1",
    session_started_ms: minutes(60), last_activity_ms: minutes(2), agents: [{ id: "project-agent", watched: true, unread_count: 0 }] };

  const READ_MARK = "te-01K0000000";
  const byAgent = { kind: "agent", agent_id: "builder" };
  const task = (number, title, over = {}) => ({
    id: `task-${number}`, project_id: "p-1", number, title, body: "", state: "open", status: "backlog",
    labels: [], priority: "none", assignee: null, trackers: [], identities: {}, attachments: [],
    links: { workspace_ids: ["ws-1"], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
    created_by: { kind: "user" }, created_at: iso(minutes(300)), updated_at: iso(minutes(20 + number)), closed_at: null,
    read_through: READ_MARK, watched: true, unread_count: 0, ...over,
  });
  const tasks = [
    task(12, "Kanban drag does not persist", { status: "in_progress", assignee: byAgent,
      identities: { builder: { agent_id: "builder", name: "Builder", ordinal: 1, workspace_id: "ws-1", workspace_name: "Checkout flow", available: true } } }),
    task(7, "Decide the dashboard's empty copy", { status: "in_review", assignee: { kind: "user" }, unread_count: 1 }),
    task(5, "Toolbar tab badge spacing", { status: "ready" }),
    task(3, "Rail badge sizing", { status: "done" }),
    task(2, "Drop the legacy plan view", { state: "closed", status: "backlog", closed_at: iso(minutes(40)) }),
    task(1, "Ship the first board", { state: "closed", status: "done", closed_at: iso(minutes(60)) }),
  ];
  const timelineOf = (one) => [
    { type: "event", id: READ_MARK, task_id: one.id, at: one.created_at, actor: { kind: "user" }, kind: "created", payload: {} },
    ...Array.from({ length: one.unread_count || 0 }, (_, index) => ({
      type: "comment", id: `tc-01K000000${index + 1}`, task_id: one.id, author: byAgent,
      body: "Ready for your look.", refs: [], created_at: iso(minutes(10 - index)),
    })),
  ];
  const columns = [
    { id: "backlog", name: "Backlog" }, { id: "ready", name: "Ready" }, { id: "in_progress", name: "In progress" },
    { id: "in_review", name: "In review" }, { id: "done", name: "Done" },
  ];
  const userSession = { session_started_ms: minutes(30), last_activity_ms: minutes(1), previous_session_ended_ms: minutes(600), gap_ms: 21_600_000, now_ms: now };
  const answers = {
    "tasks.list": () => ({ project_id: "p-1", tasks, user_session: userSession }),
    "tasks.columns": () => ({ columns }),
    "tasks.get": ({ task_id: id }) => {
      const one = tasks.find((candidate) => candidate.id === id);
      return { task: one, timeline: timelineOf(one) };
    },
  };
  const call = async (method, params = {}) => (answers[method] ? answers[method](params) : {});
  await events.greetBridge(async () => ({
    api_version: "2.0.0", push_events: true,
    capabilities: ["changes.subscriptions", "tasks.watching", "tasks.attachments", "tasks.context", "tasks.commentUserNotifies"],
    changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"], items: "bodies" },
  }), { deviceId: device });

  app.App.gated = false;
  app.App.devices = [{ id: device, name: "workshop", status: "online" }];
  app.App.selectedDeviceId = device;
  await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
  const view = merge.liveFeedSnapshot({ items: [workspaceRun, projectRun], runs: [workspaceRun, projectRun] },
    { projects: [project] }, { workspaces: [workspace] }, device);
  await cache.writeCached({ deviceId: device, entityId: "", kind: "feed" }, view);
  await cache.writeCached({ deviceId: device, entityId: "", kind: "projects" }, view.projects);
  await cache.writeCached({ deviceId: device, entityId: "", kind: "workspaces" }, view.workspaces);
  await trackerCache.writeTasksRecord(device, "p-1", trackerCache.tasksRecord(tasks, columns));
  contexts.adoptDeviceSession({ deviceId: device, call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
  await feed.startFeed();
  if (inbox) {
    app.App.route = { name: "inbox" };
    await shell.initInboxRail();
    return;
  }
  document.body.classList.add("inbox-collapsed");
  await toolbar.initToolbar();
  app.initRouter();
  location.hash = hash;
  app.App.route = router.routeFromHash(hash);
  app.render();
}

const MODULES = {
  app: "src/app.js", cache: "src/core/localCache.js", feed: "src/core/taskFeed.js", merge: "src/core/feedMerge.js",
  contexts: "src/core/deviceContexts.js", toolbar: "src/core/toolbar.js", events: "src/core/changeEvents.js",
  router: "src/core/router.js", trackerCache: "src/core/trackerCache.js", shell: "src/core/inboxShell.js",
};

/** The app's own page head over the app's body, in the theme asked for,
 *  served as a page of its own so the sheets read as UTF-8. */
async function mountApp(page, basePath, theme) {
  const url = new URL(`${basePath}task-checkbox-capture.html`, page.url()).href;
  await page.route(url, (route) => route.fulfill({
    contentType: "text/html; charset=utf-8",
    body: `<!doctype html><html data-theme="${theme}"><head><meta charset="utf-8">
      <meta name="viewport" content="width=device-width, initial-scale=1">
      <script>localStorage.setItem("build.theme", "${theme}");</script>
      <link rel="stylesheet" href="${basePath}src/styles.css">
      <link rel="stylesheet" href="${basePath}src/styles/shell.css"></head><body>${bodyHtml}</body></html>`,
  }));
  await page.goto(url, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
}

async function open(page, basePath, theme, { hash = "", inbox = false } = {}) {
  page.on("pageerror", (error) => console.error("pageerror:", error.message));
  await mountApp(page, basePath, theme);
  // The app first, the way main.js imports it: its module graph has cycles
  // that only settle in that order.
  await loadBrowserModules(page, { app: MODULES.app }, basePath);
  // The loader waits for window.__layoutModules, which the first load set.
  await page.evaluate(() => { delete window.__layoutModules; });
  await loadBrowserModules(page, MODULES, basePath);
  await page.evaluate(seed, { device: DEVICE, hash, inbox });
  const stamped = await page.evaluate(() => document.documentElement.getAttribute("data-theme"));
  if (stamped !== theme) throw new Error(`the page stands in ${stamped}, wanted ${theme}`);
}

/** Wait for `count` of `selector`: a paint follows the cache write that asked
 *  for it, so the capture waits on the page, not a timer. */
async function expectCount(page, selector, count, what) {
  let got = 0;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    got = await page.locator(selector).count();
    if (got === count) return;
    await page.waitForTimeout(100);
  }
  throw new Error(`${what}: ${got} of ${selector}, wanted ${count}`);
}

/** Two marks drawn in the same colour: open/closed, whatever the shape. */
async function expectSameColour(page, first, second, what) {
  const colour = (selector) => page.locator(selector).first().evaluate((node) => getComputedStyle(node).color);
  const [a, b] = [await colour(first), await colour(second)];
  if (a !== b) throw new Error(`${what}: ${a} and ${b}, wanted one colour`);
}

/** Every mark on the page is a checkbox, never the circle. */
async function expectNoCircle(page, where) {
  const circles = await page.locator(".task-state svg circle, #dir-rail [data-tab=tasks] svg circle").count();
  if (circles) throw new Error(`${where}: ${circles} circle(s) in a task mark`);
}

const settle = async (page) => {
  await page.waitForTimeout(300);
  await page.mouse.move(0, 0);
};

for (const theme of ["dark", "light"]) {
  // ---- the board: an empty box, a checked one and a slashed one -------------
  await withLayoutPage(async ({ page, basePath }) => {
    await open(page, basePath, theme, { hash: `#/device/${DEVICE}/project/p-1?view=board` });
    await expectCount(page, ".task-card .task-state-open svg.lucide-square", 3, "open cards");
    await expectCount(page, ".task-card .task-mark-done svg.lucide-square-check", 2, "done cards");
    await expectCount(page, ".task-card .task-state-closed.task-mark-done", 1, "closed done card");
    await expectSameColour(page, '[data-task="task-1"] .task-state', '[data-task="task-2"] .task-state', "closed cards");
    await expectNoCircle(page, "board");
    await settle(page);
    await page.screenshot({ path: `${output}/board-${theme}.png` });
    await page.locator('[data-task="task-3"]').first().screenshot({ path: `${output}/board-card-done-${theme}.png` });
    await page.locator('[data-task="task-12"]').first().screenshot({ path: `${output}/board-card-open-${theme}.png` });
    await page.locator('[data-task="task-2"]').first().screenshot({ path: `${output}/board-card-closed-${theme}.png` });
    await page.locator('[data-task="task-1"]').first().screenshot({ path: `${output}/board-card-closed-done-${theme}.png` });
  }, { width: 1280, height: 720, deviceScaleFactor: 2 });

  // ---- a task's own page, for each of the three ----------------------------
  const pages = [[12, "open", "Open"], [3, "done", "Open"], [1, "closed-done", "Closed"], [2, "closed", "Closed"]];
  for (const [number, mark, word] of pages) {
    await withLayoutPage(async ({ page, basePath }) => {
      await open(page, basePath, theme, { hash: `#/device/${DEVICE}/project/p-1/tasks/task-${number}` });
      await expectCount(page, `.task-page-head .task-state-${word.toLowerCase()}`, 1, `task #${number} page mark`);
      const said = await page.locator(".task-page-state").textContent();
      if (said !== word) throw new Error(`task #${number}: the head says ${said}, wanted ${word}`);
      await expectNoCircle(page, `task #${number}`);
      await settle(page);
      await page.screenshot({ path: `${output}/task-page-${mark}-${theme}.png` });
      await page.locator(".task-state").first().locator("xpath=..").screenshot({ path: `${output}/task-page-head-${mark}-${theme}.png` });
    }, { width: 1280, height: 720, deviceScaleFactor: 2 });
  }

  // ---- the workspace rail's Tasks face -------------------------------------
  await withLayoutPage(async ({ page, basePath }) => {
    await open(page, basePath, theme, { hash: `#/device/${DEVICE}/project/p-1/workspace/ws-1/tasks` });
    await expectCount(page, "#dir-rail [data-tab=tasks] svg.lucide-square-check", 1, "rail Tasks face");
    await expectNoCircle(page, "workspace rail");
    await settle(page);
    await page.screenshot({ path: `${output}/workspace-tasks-${theme}.png` });
    await page.locator("#dir-rail").screenshot({ path: `${output}/workspace-rail-${theme}.png` });
  }, { width: 1280, height: 720, deviceScaleFactor: 2 });

  // ---- the dashboard's Needs you, and the inbox ----------------------------
  await withLayoutPage(async ({ page, basePath }) => {
    await open(page, basePath, theme, { hash: `#/device/${DEVICE}/project/p-1` });
    await expectCount(page, '[data-task="task-7"]', 1, "Needs you row");
    await expectNoCircle(page, "dashboard");
    await settle(page);
    await page.screenshot({ path: `${output}/dashboard-needs-you-${theme}.png` });
  }, { width: 1280, height: 720 });
  await withLayoutPage(async ({ page, basePath }) => {
    await open(page, basePath, theme, { inbox: true });
    await expectCount(page, '#inbox-list .inbox-entry[data-key^="tracker_task:"]', 1, "inbox task row");
    await settle(page);
    await page.locator("#inbox-rail").screenshot({ path: `${output}/inbox-${theme}.png` });
  }, { width: 1280, height: 520 });
}

console.log(`captured into ${output}`);
