// Capture the review images for #183 in a real Chromium against the production
// app, at a phone's width: the inbox popover on the projects face, with the top
// badge beside the project heads. The fixture is the report's: two projects,
// a Needs-you task in each with nothing unread, and a watched task nobody
// holds with 2 unread (its created and tracked events) and no row.
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
  // Filed and tracked by an agent after the read mark: 2 unread, no row.
  const bookkeeping = (one) => ["created", "tracked"].map((kind, index) => ({
    type: "event", id: `te-01K000000${index + 1}`, task_id: one.id, at: iso(minutes(30 - index)),
    actor: { kind: "agent", agent_id: "filer" }, kind, payload: {},
  }));
  const tasksOf = {
    build: [
      task("build", 159, "Tighten Needs you", { status: "in_review" }),
      task("build", 113, "Milestones for tasks", { unread_count: 2 }),
    ],
    smarter: [
      task("smarter", 31, "Bot memory budget", { assignee: { kind: "user" } }),
      task("smarter", 40, "Deploy batching", { unread_count: 1 }),
    ],
  };
  const timelineOf = (one) => (one.unread_count ? bookkeeping(one).slice(0, one.unread_count) : []);
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
    capabilities: ["changes.subscriptions", "requests.priority", "errors.codes"],
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

/** Wait for the page's words to settle: a paint follows the cache write that
 *  asked for it, so the capture waits on the words, not a timer. */
async function settled(read, what) {
  let last = null;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const now = await read();
    if (now !== null && now === last) return now;
    last = now;
    await new Promise((done) => setTimeout(done, 150));
  }
  throw new Error(`${what} never settled: ${last}`);
}

await withLayoutPage(async ({ page, basePath }) => {
  page.on("pageerror", (error) => console.error("pageerror:", error.message));
  await mountApp(page, basePath);
  await loadBrowserModules(page, { app: MODULES.app }, basePath);
  await loadBrowserModules(page, MODULES, basePath);
  await page.evaluate(seed, { device: DEVICE });
  await page.locator(`#inbox-list .inbox-entry[data-key="tracker_task:task-31"]`).waitFor();
  if (!(await page.evaluate(() => document.body.classList.contains("inbox-popover-open")))) {
    await page.locator("#inbox-open").click();
  }
  await page.waitForFunction(() => document.body.classList.contains("inbox-popover-open"));
  const numbers = () => page.evaluate(() => {
    const heads = [...document.querySelectorAll("#inbox-list .inbox-project-head")]
      .map((head) => `${head.querySelector(".inbox-project-name").textContent} ${head.querySelector(".inbox-unread")?.textContent || 0}`);
    return `top ${document.querySelector("#inbox-open .inbox-open-count").textContent || 0} · ${heads.join(" · ")}`;
  });
  const said = await settled(numbers, "badges");
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${output}/inbox-popover.png` });
  console.log(said);
}, { width: 390, height: 844 });

console.log(`captured into ${output}`);
