// Capture the review images for #103 with the production inbox rail: the
// workspace row's running count and unread badge, the project agent's row, and
// the projects face's head expanded and folded.
// Run from spa/: node test/browser/captureInboxRows.mjs (CAPTURE_DIR to redirect)
import { mkdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const output = process.env.CAPTURE_DIR || fileURLToPath(new URL("../../../design/inbox-rows/", import.meta.url));
const directory = output.endsWith("/") ? output : `${output}/`;
const html = await readFile(fileURLToPath(new URL("../../index.html", import.meta.url)), "utf8");
const body = html.match(/<body>([\s\S]*)<\/body>/)[1];
await mkdir(directory, { recursive: true });

const DEVICE = "review-device";
const PROJECT_KEY = `${DEVICE}/review-project`;

await withLayoutPage(async ({ page, basePath }) => {
  await mountLayout(page, body, { basePath });
  await loadBrowserModules(page, {
    app: "src/app.js",
    cache: "src/core/localCache.js",
    feed: "src/core/taskFeed.js",
    inbox: "src/core/inboxShell.js",
    view: "src/core/inboxView.js",
    contexts: "src/core/deviceContexts.js",
  }, basePath);
  await page.evaluate(async ({ device }) => {
    const { App } = window.__layoutModules.app;
    const { writeCached } = window.__layoutModules.cache;
    const { startFeed } = window.__layoutModules.feed;
    const { initInboxRail } = window.__layoutModules.inbox;
    App.route = { name: "inbox" };
    App.devices = [{ id: device, name: "This computer", status: "online" }];
    App.selectedDeviceId = device;
    const now = Date.now();
    const minutes = (count) => now - count * 60_000;
    const agent = (id, over = {}) => ({ id, watched: true, working: false, unread_count: 0, ...over });
    // Stamped as one machine's snapshot stamps them (core/deviceKey.js).
    const projectKey = `${device}/review-project`;
    const stamp = { deviceId: device, projectKey };
    const project = { id: "review-project", name: "Build", entity_id: "project-run",
      session_started_ms: minutes(90), last_activity_ms: minutes(2), ...stamp };
    const base = { project_id: "review-project", managed: true, ...stamp,
      directories: [{ id: "src", source_id: "review-source", is_git: true }] };
    const ready = { ...base, id: "inbox-rows", workspaceKey: `${device}/inbox-rows`, name: "Inbox rows", status: "ready", can_finish: true,
      entity_id: "inbox-run", session_started_ms: minutes(80), last_activity_ms: minutes(3),
      work_summary: { pushes: 2, behind: 0, additions: 184, deletions: 37 } };
    const busy = { ...base, id: "rail-overview", workspaceKey: `${device}/rail-overview`, name: "Rail overview", status: "ready", can_finish: false,
      finish_blockers: ["agent_working"], entity_id: "rail-run", session_started_ms: minutes(40), last_activity_ms: minutes(1),
      work_summary: { pushes: 0, behind: 1, additions: 52, deletions: 8 } };
    // The row's own flags say what the bridge derives from the roster.
    const inboxRun = { kind: "branch", run_id: "inbox-run", project_id: "review-project", ...stamp,
      unread: true, working: true, agents: [
      agent("builder", { working: true, unread_count: 2 }),
      agent("reviewer", { working: true, unread_count: 4, watched: false }),
      agent("tester", { unread_count: 1 }),
    ] };
    const railRun = { kind: "branch", run_id: "rail-run", project_id: "review-project", ...stamp,
      working: true, agents: [
      agent("rail", { working: true }),
    ] };
    const projectRun = { kind: "branch", run_id: "project-run", project_id: "review-project", ...stamp,
      session_started_ms: minutes(60), last_activity_ms: minutes(2),
      agents: [agent("project-agent", { unread_count: 2 })] };
    await writeCached({ deviceId: device, entityId: "", kind: "projects" }, [project]);
    await writeCached({ deviceId: device, entityId: "", kind: "workspaces" }, [ready, busy]);
    await writeCached({ deviceId: device, entityId: "", kind: "feed" },
      { items: [inboxRun, railRun, projectRun], runs: [inboxRun, railRun, projectRun],
        projects: [project], workspaces: [ready, busy] });
    // A paired machine that is answering, so the rows paint as they do day
    // to day rather than greyed as away.
    window.__layoutModules.contexts.adoptDeviceSession({ deviceId: device, call: async () => ({}),
      close() {}, peer() {}, onCarrier() {}, onPush() {} });
    await initInboxRail();
    await startFeed();
  }, { device: DEVICE });

  const rowKeys = () => page.$$eval("#inbox-list .inbox-entry", (rows) => rows.map((row) => row.dataset.key));
  await page.waitForFunction(() => document.querySelectorAll("#inbox-list .inbox-entry").length === 3);
  const keys = await rowKeys();
  const expected = [`workspace:${DEVICE}/inbox-rows`, `project-agent:${PROJECT_KEY}`, `workspace:${DEVICE}/rail-overview`];
  if (JSON.stringify(keys) !== JSON.stringify(expected)) throw new Error(`Unexpected inbox rows: ${keys.join(", ")}`);
  const row = (key) => page.locator(`#inbox-list .inbox-entry[data-key="${key}"]`);
  await page.locator("#inbox-rail").screenshot({ path: `${directory}inbox-face.png` });
  await row(expected[0]).screenshot({ path: `${directory}workspace-row.png` });
  await row(expected[1]).screenshot({ path: `${directory}project-agent-row.png` });

  await page.evaluate(() => window.__layoutModules.view.setInboxView("projects"));
  const head = page.locator(`#inbox-list .inbox-project[data-project="${PROJECT_KEY}"] > .inbox-project-head`);
  const headBadge = () => head.locator(".inbox-unread").textContent();
  await head.waitFor();
  if (await headBadge() !== "2") throw new Error(`Expanded head badge: ${await headBadge()}`);
  await page.locator("#inbox-rail").screenshot({ path: `${directory}projects-face-expanded.png` });
  await head.screenshot({ path: `${directory}projects-head-expanded.png` });

  await head.locator("[data-project-fold]").click();
  await page.waitForFunction((key) => document.querySelector(`#inbox-list .inbox-project[data-project="${key}"] .inbox-project-head .inbox-unread`)?.textContent === "5", PROJECT_KEY);
  await page.evaluate(() => document.activeElement?.blur());
  await page.mouse.move(0, 0);
  await page.locator("#inbox-rail").screenshot({ path: `${directory}projects-face-collapsed.png` });
  await head.screenshot({ path: `${directory}projects-head-collapsed.png` });
}, { width: 1280, height: 520 });
