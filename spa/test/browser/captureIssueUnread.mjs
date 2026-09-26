// Capture the review images for #104 in a real Chromium against the production
// app: a watched issue's unread on the inbox's workspace and project badges
// (both faces), the project's Issues tab in the toolbar, the workspace's Issues
// face on its rail, and the per-issue bubble on the dashboard, list and board.
// One scripted machine answers the tracker; everything else is the app's own.
// Run from spa/: node test/browser/captureIssueUnread.mjs [output directory]
import { mkdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

const output = resolve(process.argv[2] || "../design/issue-unread");
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

  // Each issue's timeline: filed by the user, then `unread` agent comments
  // after the read mark, so the cached timeline and the list agree.
  const READ_MARK = "ie-01K0000000";
  const issue = (number, title, over = {}) => ({
    id: `issue-${number}`, project_id: "p-1", number, title, body: "", state: "open", status: "backlog",
    labels: [], priority: "none", assignee: null, trackers: [], identities: {}, attachments: [],
    links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_issue_id: null },
    created_by: { kind: "user" }, created_at: iso(minutes(300)), updated_at: iso(minutes(20 + number)), closed_at: null,
    read_through: READ_MARK, ...over,
  });
  const byAgent = { kind: "agent", agent_id: "builder" };
  const issues = [
    issue(12, "Kanban drag does not persist", { status: "in_progress", assignee: byAgent, watched: true, unread_count: 3,
      identities: { builder: { agent_id: "builder", name: "Builder", ordinal: 1, workspace_id: "ws-1", workspace_name: "Checkout flow", available: true } } }),
    issue(7, "Decide the dashboard's empty copy", { status: "ready", assignee: { kind: "user" }, watched: true, unread_count: 2 }),
    issue(5, "Toolbar tab badge spacing", { watched: true, unread_count: 1 }),
    issue(9, "Board columns forget their order", { unread_count: undefined }),
    issue(3, "Rail badge sizing", { status: "done", watched: true, unread_count: 0 }),
  ];
  const timelineOf = (one) => [
    { type: "event", id: READ_MARK, issue_id: one.id, at: one.created_at, actor: { kind: "user" }, kind: "created", payload: {} },
    ...Array.from({ length: one.unread_count || 0 }, (_, index) => ({
      type: "comment", id: `ic-01K000000${index + 1}`, issue_id: one.id, author: byAgent,
      body: "An update.", refs: [], created_at: iso(minutes(10 - index)),
    })),
  ];
  const columns = [
    { id: "backlog", name: "Backlog" }, { id: "ready", name: "Ready" }, { id: "in_progress", name: "In progress" },
    { id: "in_review", name: "In review" }, { id: "done", name: "Done" },
  ];
  const userSession = { session_started_ms: minutes(30), last_activity_ms: minutes(1), previous_session_ended_ms: minutes(600), gap_ms: 21_600_000, now_ms: now };
  const answers = {
    "issues.list": () => ({ project_id: "p-1", issues, user_session: userSession }),
    "issues.columns": () => ({ columns }),
    "issues.get": ({ issue_id: id }) => {
      const one = issues.find((candidate) => candidate.id === id);
      return { issue: one, timeline: timelineOf(one) };
    },
  };
  const call = async (method, params = {}) => (answers[method] ? answers[method](params) : {});
  await events.greetBridge(async () => ({
    api_version: "1.21.0", push_events: true,
    changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "issues"], items: "bodies" },
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
  await trackerCache.writeIssuesRecord(device, "p-1", trackerCache.issuesRecord(issues, columns));
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
  const url = new URL(`${basePath}issue-unread-capture.html`, page.url()).href;
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
  await expectText(head.locator(".inbox-unread"), "4", "expanded head");
  await page.locator("#inbox-rail").screenshot({ path: `${output}/projects-face-expanded.png` });
  await head.locator("[data-project-fold]").click();
  await expectText(head.locator(".inbox-unread"), "10", "folded head");
  await page.evaluate(() => document.activeElement?.blur());
  await page.mouse.move(0, 0);
  await page.locator("#inbox-rail").screenshot({ path: `${output}/projects-face-folded.png` });
}, { width: 1280, height: 520 });

// ---- the project's Issues tab, and the bubble on each view -------------------
for (const [view, query] of [["dashboard", ""], ["list", "?view=list"], ["board", "?view=board"]]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await open(page, basePath, { hash: `#/device/${DEVICE}/project/p-1${query}` });
    await expectText(page.locator('#toolbar [data-project-tab="issues"] .issue-unread'), "6", "project Issues tab");
    if (view === "dashboard") {
      await expectText(page.locator('[data-issue="issue-7"] .issue-unread'), "2", "dashboard bubble");
    } else {
      await expectText(page.locator('[data-issue="issue-12"] .issue-unread'), "3", `${view} bubble`);
      if (await page.locator('[data-issue="issue-9"] .issue-unread').count()) throw new Error(`${view}: an unwatched issue wears a bubble`);
    }
    await page.waitForTimeout(300);
    await page.mouse.move(0, 0);
    await page.screenshot({ path: `${output}/project-issues-${view}.png` });
    if (view === "dashboard") {
      await page.locator("#toolbar .toolbar").screenshot({ path: `${output}/project-issues-tab.png` });
      await page.locator('[data-dashboard-tab="inProgress"]').click();
      await expectText(page.locator('[data-issue="issue-12"] .issue-unread'), "3", "dashboard in progress bubble");
      await page.waitForTimeout(200);
      await page.screenshot({ path: `${output}/project-issues-dashboard-in-progress.png` });
    }
  }, { width: 1280, height: 720 });
}

// ---- the workspace's Issues face ---------------------------------------------
await withLayoutPage(async ({ page, basePath }) => {
  await open(page, basePath, { hash: `#/device/${DEVICE}/project/p-1/workspace/ws-1/issues` });
  const face = page.locator("#dir-rail [data-tab=issues]");
  // #12 alone: the one watched issue an agent of this workspace holds.
  await expectText(face.locator(".dirtab-count"), "3", "workspace Issues face");
  await expectText(page.locator('[data-issue="issue-12"] .issue-unread'), "3", "workspace tab bubble");
  await page.waitForTimeout(300);
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${output}/workspace-issues.png` });
  await page.locator("#dir-rail").screenshot({ path: `${output}/workspace-issues-face.png` });
}, { width: 1280, height: 720 });

console.log(`captured into ${output}`);
