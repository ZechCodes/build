// Capture the review images for #256 in a real Chromium against the production
// app: web links in markdown — `[text](url)` and a bare https URL — rendered as
// accent links with a ↗ after them, beside a task reference (no ↗) and a code
// span that stays literal. One agent message in a workspace's chat and one
// comment on a task's page, each in the dark theme and the light one, cropped
// to the message with a little context around it.
// Run from spa/: node test/browser/captureWebLinks.mjs [output directory]
import { mkdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

const output = resolve(process.argv[2] || "/tmp/wl-shots");
await mkdir(output, { recursive: true });
const indexHtml = await readFile(new URL("../../index.html", import.meta.url), "utf8");
const bodyHtml = indexHtml.match(/<body>([\s\S]*)<\/body>/)[1];
const DEVICE = "dev-1";

const CHAT_BODY = "See [OpenAI's documentation](https://developers.openai.com/api/docs/models/gpt-6.1-sol) and the spec at "
  + "https://spec.commonmark.org/0.31.2/#links. Task #1 is beside them, and `[literal](https://x.test)` stays code.";
const COMMENT_BODY = "Checked against [OpenAI's documentation](https://developers.openai.com/api/docs/models/gpt-6.1-sol) "
  + "and the disambiguation page (https://en.wikipedia.org/wiki/Markdown_(disambiguation)). This unblocks #5.";

/** Runs in the page: the machine's records, a chat thread and a comment, and
 *  a session answering them, then the app stands on `hash`. */
async function seed({ device, hash, chatBody, commentBody }) {
  const { app, cache, feed, merge, contexts, toolbar, events, router, trackerCache } = window.__layoutModules;
  const now = Date.now();
  const minutes = (count) => now - count * 60_000;
  const iso = (ms) => new Date(ms).toISOString();
  const agent = { id: "builder", name: "Builder", ordinal: 1, watched: true, working: false, unread_count: 0,
    state: "live", provider: "claude_adk", conversation_id: "conversation-builder" };
  const project = { project_id: "p-1", id: "p-1", name: "Build", entity_id: "project-run",
    session_started_ms: minutes(90), last_activity_ms: minutes(2) };
  const workspace = { id: "ws-1", workspace_id: "ws-1", project_id: "p-1", name: "Web links", status: "ready",
    managed: true, can_finish: true, entity_id: "run-1", session_started_ms: minutes(80), last_activity_ms: minutes(3),
    directories: [{ source_id: "build", name: "Build", is_git: true }],
    work_summary: { pushes: 1, behind: 0, additions: 200, deletions: 33 } };
  const workspaceRun = { kind: "branch", run_id: "run-1", project_id: "p-1", unread: false, working: false, agents: [agent] };
  const projectRun = { kind: "branch", run_id: "project-run", project_id: "p-1",
    session_started_ms: minutes(60), last_activity_ms: minutes(2), agents: [{ id: "project-agent", watched: true, unread_count: 0 }] };

  const byAgent = { kind: "agent", agent_id: "builder" };
  const task = (number, title, over = {}) => ({
    id: `task-${number}`, project_id: "p-1", number, title, body: "", state: "open", status: "backlog",
    labels: [], priority: "none", assignee: null, trackers: [], identities: {}, attachments: [],
    links: { workspace_ids: ["ws-1"], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
    created_by: { kind: "user" }, created_at: iso(minutes(300)), updated_at: iso(minutes(20 + number)), closed_at: null,
    read_through: "tc-01K0000009", watched: true, unread_count: 0, ...over,
  });
  const tasks = [
    task(12, "Markdown web links render as links", { status: "in_progress", assignee: byAgent }),
    task(5, "Toolbar tab badge spacing", { status: "ready" }),
    task(1, "Ship the first board", { state: "closed", status: "done", closed_at: iso(minutes(60)) }),
  ];
  const timelineOf = (one) => [
    { type: "event", id: "te-01K0000000", task_id: one.id, at: one.created_at, actor: { kind: "user" }, kind: "created", payload: {} },
    { type: "comment", id: "tc-01K0000009", task_id: one.id, author: byAgent, body: commentBody, refs: [], created_at: iso(minutes(8)) },
  ];
  const columns = [
    { id: "backlog", name: "Backlog" }, { id: "ready", name: "Ready" }, { id: "in_progress", name: "In progress" },
    { id: "in_review", name: "In review" }, { id: "done", name: "Done" },
  ];
  const thread = { items: [
    { type: "message", data: { id: "m1", sequence: 1, role: "user", body: "Where are the model docs?", created_at: iso(minutes(6)) } },
    { type: "message", data: { id: "m2", sequence: 2, role: "agent", body: chatBody, created_at: iso(minutes(5)) } },
  ] };
  const userSession = { session_started_ms: minutes(30), last_activity_ms: minutes(1), previous_session_ended_ms: minutes(600), gap_ms: 21_600_000, now_ms: now };
  const answers = {
    "tasks.list": () => ({ project_id: "p-1", tasks, user_session: userSession }),
    "tasks.columns": () => ({ columns }),
    "tasks.get": ({ task_id: id }) => {
      const one = tasks.find((candidate) => candidate.id === id);
      return { task: one, timeline: timelineOf(one) };
    },
    "models.list": () => ({ default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }),
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
  await cache.writeCached({ deviceId: device, entityId: "run-1", kind: "thread", sub: "builder" }, thread);
  await trackerCache.writeTasksRecord(device, "p-1", trackerCache.tasksRecord(tasks, columns));
  contexts.adoptDeviceSession({ deviceId: device, call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
  await feed.startFeed();
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
  router: "src/core/router.js", trackerCache: "src/core/trackerCache.js",
};

/** The app's own page head over the app's body, in the theme asked for. */
async function mountApp(page, basePath, theme) {
  const url = new URL(`${basePath}web-links-capture.html`, page.url()).href;
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

async function open(page, basePath, theme, hash) {
  page.on("pageerror", (error) => console.error("pageerror:", error.message));
  await mountApp(page, basePath, theme);
  // The app first, the way main.js imports it: its module graph has cycles.
  await loadBrowserModules(page, { app: MODULES.app }, basePath);
  await page.evaluate(() => { delete window.__layoutModules; });
  await loadBrowserModules(page, MODULES, basePath);
  await page.evaluate(seed, { device: DEVICE, hash, chatBody: CHAT_BODY, commentBody: COMMENT_BODY });
  const stamped = await page.evaluate(() => document.documentElement.getAttribute("data-theme"));
  if (stamped !== theme) throw new Error(`the page stands in ${stamped}, wanted ${theme}`);
}

/** The element holding `text`, deepest first, once the page paints it. */
async function waitForText(page, text, what) {
  const locator = page.locator(`:text("${text}")`).last();
  try {
    await locator.waitFor({ timeout: 10_000 });
  } catch (error) {
    throw new Error(`${what}: "${text}" never painted (${error.message})`);
  }
  return locator;
}

/** The links inside `container`: the web ones wear the class, the task one does not. */
async function describeLinks(container) {
  return container.evaluate((node) => [...node.querySelectorAll("a")].map((link) => ({
    text: link.textContent, href: link.getAttribute("href"), cls: link.className, target: link.target,
    colour: getComputedStyle(link).color, after: getComputedStyle(link, "::after").content,
  })));
}

/** Screenshot `container` plus `margin` px of the page around it. */
async function shootAround(page, container, path, margin = 24) {
  await container.scrollIntoViewIfNeeded();
  const box = await container.boundingBox();
  const viewport = page.viewportSize();
  const x = Math.max(0, box.x - margin);
  const y = Math.max(0, box.y - margin);
  const clip = { x, y, width: Math.min(viewport.width - x, box.width + margin * 2),
    height: Math.min(viewport.height - y, box.height + margin * 2) };
  await page.screenshot({ path, clip });
}

const settle = async (page) => {
  await page.waitForTimeout(400);
  await page.mouse.move(0, 0);
};

for (const theme of ["dark", "light"]) {
  // ---- an agent's message in the workspace chat ----------------------------
  await withLayoutPage(async ({ page, basePath }) => {
    await open(page, basePath, theme, `#/device/${DEVICE}/project/p-1/workspace/ws-1?agent=builder`);
    const text = await waitForText(page, "stays code", "chat message");
    await page.locator("a.md-link").first().waitFor({ timeout: 10_000 });
    const message = text.locator("xpath=ancestor-or-self::*[.//a[contains(@class,'md-link')]][1]");
    console.log("chat", theme, JSON.stringify(await describeLinks(message), null, 1));
    await settle(page);
    await shootAround(page, message, `${output}/chat-${theme}.png`);
    await page.screenshot({ path: `${output}/chat-full-${theme}.png` });
  }, { width: 1280, height: 800, deviceScaleFactor: 2 });

  // ---- a comment on a task's page ------------------------------------------
  await withLayoutPage(async ({ page, basePath }) => {
    await open(page, basePath, theme, `#/device/${DEVICE}/project/p-1/tasks/task-12`);
    const text = await waitForText(page, "This unblocks", "task comment");
    await page.locator("a.md-link").first().waitFor({ timeout: 10_000 });
    const comment = text.locator("xpath=ancestor-or-self::li[contains(@class,'task-comment')][1]");
    console.log("comment", theme, JSON.stringify(await describeLinks(comment), null, 1));
    await settle(page);
    await shootAround(page, comment, `${output}/comment-${theme}.png`);
    await page.screenshot({ path: `${output}/comment-full-${theme}.png` });
  }, { width: 1280, height: 800, deviceScaleFactor: 2 });
}

console.log(`captured into ${output}`);
