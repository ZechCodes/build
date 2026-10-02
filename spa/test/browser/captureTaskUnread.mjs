// Capture the review images for #104 in a real Chromium against the production
// app: a watched task's unread on the inbox's workspace and project badges
// (both faces), the project's Tasks face on its rail, the workspace's Tasks
// face on its rail, and the per-task bubble on the dashboard, list and board.
// One scripted machine answers the tracker; everything else is the app's own.
// Run from spa/: node test/browser/captureTaskUnread.mjs [output directory]
import { mkdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

const output = resolve(process.argv[2] || "../design/task-unread");
await mkdir(output, { recursive: true });
const indexHtml = await readFile(new URL("../../index.html", import.meta.url), "utf8");
const bodyHtml = indexHtml.match(/<body>([\s\S]*)<\/body>/)[1];
const DEVICE = "dev-1";
const PROJECT_KEY = `${DEVICE}/p-1`;

/** Runs in the page: the machine's records and a session answering them,
 *  then `stand` puts the app where the capture wants it. */
async function seed({ device, hash, inbox }) {
  const { app, cache, feed, merge, contexts, toolbar, events, router, trackerCache, shell } = window.__layoutModules;
  const now = Date.now();
  const minutes = (count) => now - count * 60_000;
  const iso = (ms) => new Date(ms).toISOString();
  const agent = (id, over = {}) => ({ id, watched: true, working: false, unread_count: 0, ...over });
  const project = { project_id: "p-1", id: "p-1", name: "Build", entity_id: "project-run",
    session_started_ms: minutes(90), last_activity_ms: minutes(2) };
  const workspace = { id: "ws-1", workspace_id: "ws-1", project_id: "p-1", name: "Checkout flow", status: "ready",
    managed: true, can_finish: true, entity_id: "run-1", session_started_ms: minutes(80), last_activity_ms: minutes(3),
    directories: [{ source_id: "build", name: "Build", is_git: true }],
    work_summary: { pushes: 2, behind: 0, additions: 184, deletions: 37 } };
  const workspaceRun = { kind: "branch", run_id: "run-1", project_id: "p-1", unread: true, working: true,
    agents: [agent("builder", { working: true, unread_count: 2, name: "Builder" }), agent("tester", { unread_count: 1, name: "Tester" })] };
  const projectRun = { kind: "branch", run_id: "project-run", project_id: "p-1",
    session_started_ms: minutes(60), last_activity_ms: minutes(2), agents: [agent("project-agent", { unread_count: 1 })] };

  // Each task's timeline: filed by the user, then `unread` agent comments
  // after the read mark, so the cached timeline and the list agree.
  const READ_MARK = "te-01K0000000";
  const task = (number, title, over = {}) => ({
    id: `task-${number}`, project_id: "p-1", number, title, body: "", state: "open", status: "backlog",
    labels: [], priority: "none", assignee: null, trackers: [], identities: {}, attachments: [],
    links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
    created_by: { kind: "user" }, created_at: iso(minutes(300)), updated_at: iso(minutes(20 + number)), closed_at: null,
    read_through: READ_MARK, ...over,
  });
  const byAgent = { kind: "agent", agent_id: "builder" };
  const tasks = [
    task(12, "Kanban drag does not persist", { status: "in_progress", assignee: byAgent, watched: true, unread_count: 3,
      identities: { builder: { agent_id: "builder", name: "Builder", ordinal: 1, workspace_id: "ws-1", workspace_name: "Checkout flow", available: true } } }),
    task(7, "Decide the dashboard's empty copy", { status: "ready", assignee: { kind: "user" }, watched: true, unread_count: 2 }),
    task(5, "Toolbar tab badge spacing", { watched: true, unread_count: 1 }),
    task(9, "Board columns forget their order", { unread_count: undefined }),
    task(3, "Rail badge sizing", { status: "done", watched: true, unread_count: 0 }),
  ];
  const timelineOf = (one) => [
    { type: "event", id: READ_MARK, task_id: one.id, at: one.created_at, actor: { kind: "user" }, kind: "created", payload: {} },
    ...Array.from({ length: one.unread_count || 0 }, (_, index) => ({
      type: "comment", id: `tc-01K000000${index + 1}`, task_id: one.id, author: byAgent,
      body: "An update.", refs: [], created_at: iso(minutes(10 - index)),
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
    capabilities: ["changes.subscriptions", "requests.priority", "errors.codes", "diffs.perFile", "tasks.context",
      "tasks.attachments", "tasks.watching", "conversations.settings"],
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
  inboxView: "src/core/inboxView.js",
};

/** The app's own page head over the app's body, served as a page of its own
 *  so the sheets read as UTF-8. */
async function mountApp(page, basePath) {
  const url = new URL(`${basePath}task-unread-capture.html`, page.url()).href;
  await page.route(url, (route) => route.fulfill({
    contentType: "text/html; charset=utf-8",
    body: `<!doctype html><html><head><meta charset="utf-8">
      <meta name="viewport" content="width=device-width, initial-scale=1">
      <link rel="stylesheet" href="${basePath}src/styles.css">
      <link rel="stylesheet" href="${basePath}src/styles/shell.css"></head><body>${bodyHtml}</body></html>`,
  }));
  await page.goto(url, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
}

async function open(page, basePath, { hash = "", inbox = false } = {}) {
  page.on("pageerror", (error) => console.error("pageerror:", error.message));
  await mountApp(page, basePath);
  // The app first, the way main.js imports it: its module graph has cycles
  // that only settle in that order.
  await loadBrowserModules(page, { app: MODULES.app }, basePath);
  // The harness waits for the set to appear; the app's own set is already
  // there, so it is taken away or the wait ends before the rest have loaded.
  await page.evaluate(() => delete window.__layoutModules);
  await loadBrowserModules(page, MODULES, basePath);
  await page.evaluate(seed, { device: DEVICE, hash, inbox });
}

/** Wait for what the page says to settle on `want`: a paint follows the cache
 *  write that asked for it, so the capture waits on the words, not a timer. */
async function expectText(locator, want, what) {
  let got = null;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    got = (await locator.count()) ? (await locator.first().textContent()).trim() : null;
    if (got === want) return;
    await locator.page().waitForTimeout(100);
  }
  throw new Error(`${what}: ${got}, wanted ${want}`);
}

// ---- the inbox ------------------------------------------------------------
await withLayoutPage(async ({ page, basePath }) => {
  await open(page, basePath, { inbox: true });
  const row = (key) => page.locator(`#inbox-list .inbox-entry[data-key="${key}"]`);
  const workspaceRow = row(`workspace:${DEVICE}/ws-1`);
  const agentRow = row(`project-agent:${PROJECT_KEY}`);
  // The workspace: its watched agents' 2 + 1, and #12 its builder holds, 3.
  await expectText(workspaceRow.locator(".inbox-unread"), "6", "workspace badge");
  // The project: its agent's 1, #7 the user holds, 2, and #5 nobody holds, 1.
  await expectText(agentRow.locator(".inbox-unread"), "4", "project badge");
  await page.mouse.move(0, 0);
  await page.locator("#inbox-rail").screenshot({ path: `${output}/inbox-face.png` });
  await workspaceRow.screenshot({ path: `${output}/inbox-workspace-row.png` });
  await agentRow.screenshot({ path: `${output}/inbox-project-agent-row.png` });

  await page.evaluate(() => window.__layoutModules.inboxView.setInboxView("projects"));
  const head = page.locator(`#inbox-list .inbox-project[data-project="${PROJECT_KEY}"] > .inbox-project-head`);
  // A project head wears the whole block total on both faces (#183).
  await expectText(head.locator(".inbox-unread"), "10", "expanded head");
  await page.locator("#inbox-rail").screenshot({ path: `${output}/projects-face-expanded.png` });
  await head.locator("[data-project-fold]").click();
  await page.waitForFunction(() =>
    document.querySelector("[data-project-fold]")?.getAttribute("aria-expanded") === "false");
  await expectText(head.locator(".inbox-unread"), "10", "folded head");
  await page.evaluate(() => document.activeElement?.blur());
  await page.mouse.move(0, 0);
  await page.locator("#inbox-rail").screenshot({ path: `${output}/projects-face-folded.png` });
}, { width: 1280, height: 520 });

// ---- the project's Tasks tab, and the bubble on each view -------------------
for (const [view, query] of [["dashboard", ""], ["list", "?view=list"], ["board", "?view=board"]]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await open(page, basePath, { hash: `#/device/${DEVICE}/project/p-1${query}` });
    await expectText(page.locator('#dir-rail [data-tab="tasks"] .dirtab-count'), "6", "project Tasks tab");
    if (view === "dashboard") {
      await expectText(page.locator('[data-task="task-7"] .task-unread'), "2", "dashboard bubble");
    } else {
      await expectText(page.locator('[data-task="task-12"] .task-unread'), "3", `${view} bubble`);
      if (await page.locator('[data-task="task-9"] .task-unread').count()) throw new Error(`${view}: an unwatched task wears a bubble`);
    }
    await page.waitForTimeout(300);
    await page.mouse.move(0, 0);
    await page.screenshot({ path: `${output}/project-tasks-${view}.png` });
    if (view === "dashboard") {
      await page.locator("#dir-rail").screenshot({ path: `${output}/project-tasks-tab.png` });
      await page.locator('[data-dashboard-tab="active"]').click();
      await expectText(page.locator('[data-task="task-12"] .task-unread'), "3", "dashboard active bubble");
      await page.waitForTimeout(200);
      await page.screenshot({ path: `${output}/project-tasks-dashboard-active.png` });
    }
  }, { width: 1280, height: 720 });
}

// ---- the workspace's Tasks face ---------------------------------------------
await withLayoutPage(async ({ page, basePath }) => {
  await open(page, basePath, { hash: `#/device/${DEVICE}/project/p-1/workspace/ws-1/tasks` });
  const face = page.locator("#dir-rail [data-tab=tasks]");
  // #12 alone: the one watched task an agent of this workspace holds.
  await expectText(face.locator(".dirtab-count"), "3", "workspace Tasks face");
  await expectText(page.locator('[data-task="task-12"] .task-unread'), "3", "workspace tab bubble");
  await page.waitForTimeout(300);
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${output}/workspace-tasks.png` });
  await page.locator("#dir-rail").screenshot({ path: `${output}/workspace-tasks-face.png` });
}, { width: 1280, height: 720 });

console.log(`captured into ${output}`);
