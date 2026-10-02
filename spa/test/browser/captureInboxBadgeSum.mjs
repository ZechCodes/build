// Capture the review images for #183 in a real Chromium against the production
// app, at a phone's width: the inbox popover on the projects face, with the top
// badge beside the project heads. Two projects each have a fully read task
// assigned to the user, plus an unassigned watched task with unread moves
// but no Needs-you row. Their badges must sum to 3 (Build 2 + smarter-dev 1).
// Run from spa/: node test/browser/captureInboxBadgeSum.mjs [output directory]
import { mkdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

const output = resolve(process.argv[2] || "../design/inbox-badge-sum");
await mkdir(output, { recursive: true });
const indexHtml = await readFile(new URL("../../index.html", import.meta.url), "utf8");
const bodyHtml = indexHtml.match(/<body>([\s\S]*)<\/body>/)[1];
const DEVICE = "dev-1";

/** Runs in the page: the machine's records and a session answering them. */
async function seed({ device }) {
  const { app, cache, feed, merge, contexts, events, trackerCache, shell, inboxView } = window.__layoutModules;
  const now = Date.now();
  const minutes = (count) => now - count * 60_000;
  const iso = (ms) => new Date(ms).toISOString();
  const agent = (id, over = {}) => ({ id, watched: true, working: false, unread_count: 0, ...over });
  const project = (id, name) => ({ project_id: id, id, name, entity_id: `run-${id}`,
    session_started_ms: minutes(90), last_activity_ms: minutes(2) });
  const projectRun = (id) => ({ kind: "branch", run_id: `run-${id}`, project_id: id,
    session_started_ms: minutes(60), last_activity_ms: minutes(2), agents: [agent(`${id}-agent`)] });

  const READ_MARK = "te-01K0000000";
  const task = (projectId, number, title, over = {}) => ({
    id: `task-${number}`, project_id: projectId, number, title, body: "", state: "open", status: "ready",
    labels: [], priority: "none", assignee: null, trackers: [], identities: {}, attachments: [],
    links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
    created_by: { kind: "agent", agent_id: "filer" }, created_at: iso(minutes(300)), updated_at: iso(minutes(20 + number)),
    closed_at: null, watched: true, read_through: READ_MARK, unread_count: 0, ...over,
  });
  // Moves are unread news without asking the user; bookkeeping is not unread.
  const updates = (one) => ["ready", "backlog"].map((to, index) => ({
    type: "event", id: `te-01K000000${index + 1}`, task_id: one.id, at: iso(minutes(30 - index)),
    actor: { kind: "agent", agent_id: "filer" }, kind: "moved", payload: { to },
  }));
  const tasksOf = {
    build: [
      task("build", 159, "Tighten Needs you", { status: "in_review", assignee: { kind: "user" } }),
      task("build", 113, "Milestones for tasks", { unread_count: 2 }),
    ],
    smarter: [
      task("smarter", 31, "Bot memory budget", { assignee: { kind: "user" } }),
      task("smarter", 40, "Deploy batching", { unread_count: 1 }),
    ],
  };
  const timelineOf = (one) => (one.unread_count ? updates(one).slice(0, one.unread_count) : []);
  const columns = [
    { id: "backlog", name: "Backlog" }, { id: "ready", name: "Ready" }, { id: "in_progress", name: "In progress" },
    { id: "in_review", name: "In review" }, { id: "done", name: "Done" },
  ];
  const answers = {
    "tasks.list": ({ project_id: id }) => ({ project_id: id, tasks: tasksOf[id] || [] }),
    "tasks.columns": () => ({ columns }),
    "tasks.get": ({ task_id: id }) => {
      const one = Object.values(tasksOf).flat().find((candidate) => candidate.id === id);
      return { task: one, timeline: timelineOf(one) };
    },
  };
  const call = async (method, params = {}) => (answers[method] ? answers[method](params) : {});
  await events.greetBridge(async () => ({
    api_version: "2.0.0", push_events: true,
    capabilities: ["changes.subscriptions", "requests.priority", "errors.codes", "diffs.perFile", "tasks.context",
      "tasks.attachments", "tasks.watching", "tasks.commentUserNotifies", "conversations.settings"],
    changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"], items: "bodies" },
  }), { deviceId: device });

  app.App.gated = false;
  app.App.devices = [{ id: device, name: "workshop", status: "online" }];
  app.App.selectedDeviceId = device;
  await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
  const runs = [projectRun("build"), projectRun("smarter")];
  const view = merge.liveFeedSnapshot({ items: runs, runs }, { projects: [project("build", "Build"), project("smarter", "smarter-dev")] },
    { workspaces: [] }, device);
  await cache.writeCached({ deviceId: device, entityId: "", kind: "feed" }, view);
  await cache.writeCached({ deviceId: device, entityId: "", kind: "projects" }, view.projects);
  await cache.writeCached({ deviceId: device, entityId: "", kind: "workspaces" }, view.workspaces);
  for (const [id, tasks] of Object.entries(tasksOf)) {
    await trackerCache.writeTasksRecord(device, id, trackerCache.tasksRecord(tasks, columns));
    for (const one of tasks) {
      await cache.writeCached(trackerCache.taskAddress(device, id, one.id),
        trackerCache.taskRecord(one, timelineOf(one)));
    }
  }
  contexts.adoptDeviceSession({ deviceId: device, call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
  await feed.startFeed();
  app.App.route = { name: "inbox" };
  await shell.initInboxRail();
  inboxView.setInboxView("projects");
}

const MODULES = {
  app: "src/app.js", cache: "src/core/localCache.js", feed: "src/core/taskFeed.js", merge: "src/core/feedMerge.js",
  contexts: "src/core/deviceContexts.js", events: "src/core/changeEvents.js", trackerCache: "src/core/trackerCache.js",
  shell: "src/core/inboxShell.js", inboxView: "src/core/inboxView.js",
};

async function mountApp(page, basePath) {
  const url = new URL(`${basePath}inbox-badge-capture.html`, page.url()).href;
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

/** Assert the actual totals, not just a repeated (possibly empty) paint. */
async function expectBadges(page) {
  await page.waitForFunction(() => {
    const count = (selector) => document.querySelector(selector)?.textContent.trim();
    return count("#inbox-open .inbox-open-count") === "3"
      && count('[data-project="dev-1/build"] > .inbox-project-head .inbox-unread') === "2"
      && count('[data-project="dev-1/smarter"] > .inbox-project-head .inbox-unread') === "1";
  });
}

await withLayoutPage(async ({ page, basePath }) => {
  page.on("pageerror", (error) => console.error("pageerror:", error.message));
  await mountApp(page, basePath);
  await loadBrowserModules(page, { app: MODULES.app }, basePath);
  // Wait for this second module set, not the already loaded app-only set.
  await page.evaluate(() => delete window.__layoutModules);
  await loadBrowserModules(page, MODULES, basePath);
  await page.evaluate(seed, { device: DEVICE });
  if (!(await page.evaluate(() => document.body.classList.contains("inbox-popover-open")))) {
    await page.locator("#inbox-open").click();
  }
  await page.waitForFunction(() => document.body.classList.contains("inbox-popover-open"));
  for (const id of [159, 31]) {
    await page.locator(`#inbox-list .inbox-entry[data-key="tracker_task:task-${id}"]`).waitFor();
  }
  await expectBadges(page);
  for (const id of [113, 40]) {
    if (await page.locator(`#inbox-list .inbox-entry[data-key="tracker_task:task-${id}"]`).count()) {
      throw new Error(`Unassigned task ${id} unexpectedly needs the user`);
    }
  }
  const said = await page.evaluate(() => {
    const heads = [...document.querySelectorAll("#inbox-list .inbox-project-head")]
      .map((head) => `${head.querySelector(".inbox-project-name").textContent} ${head.querySelector(".inbox-unread")?.textContent || 0}`);
    return `top ${document.querySelector("#inbox-open .inbox-open-count").textContent || 0} · ${heads.join(" · ")}`;
  });
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${output}/inbox-popover.png` });
  console.log(said);
  for (const project of ["build", "smarter"]) {
    const fold = page.locator(`[data-project-fold="${DEVICE}/${project}"]`);
    await fold.click();
    await page.waitForFunction((key) =>
      document.querySelector(`[data-project-fold="${key}"]`)?.getAttribute("aria-expanded") === "false",
    `${DEVICE}/${project}`);
  }
  await expectBadges(page);
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${output}/inbox-popover-folded.png` });
}, { width: 390, height: 844 });

console.log(`captured into ${output}`);
