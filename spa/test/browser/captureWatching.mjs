// Capture the review images for #101 with the production inbox and rail.
// Run from spa/: node test/browser/captureWatching.mjs [output directory]
import { mkdir, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const output = process.argv[2]
  ? resolve(process.argv[2]) : fileURLToPath(new URL("../../../design/watching-defaults/", import.meta.url));
const html = await readFile(fileURLToPath(new URL("../../index.html", import.meta.url)), "utf8");
const body = html.match(/<body>([\s\S]*)<\/body>/)[1];
await mkdir(output, { recursive: true });

await withLayoutPage(async ({ page, basePath }) => {
  await mountLayout(page, body, { basePath });
  await loadBrowserModules(page, {
    app: "src/app.js",
    cache: "src/core/localCache.js",
    feed: "src/core/taskFeed.js",
    merge: "src/core/feedMerge.js",
    contexts: "src/core/deviceContexts.js",
    inbox: "src/core/inboxShell.js",
  }, basePath);
  await page.evaluate(async () => {
    const { App } = window.__layoutModules.app;
    const { writeCached, DEVICES_ADDRESS } = window.__layoutModules.cache;
    const { startFeed } = window.__layoutModules.feed;
    const { liveFeedSnapshot, stampRow } = window.__layoutModules.merge;
    const { initInboxRail } = window.__layoutModules.inbox;
    App.route = { name: "inbox" };
    App.devices = [{ id: "review-device", name: "This computer", status: "online" }];
    App.selectedDeviceId = "review-device";
    await writeCached(DEVICES_ADDRESS, App.devices);
    const base = { project_id: "review-project", status: "ready", managed: true,
      directories: [{ id: "src", source_id: "review-source", is_git: true }],
      work_summary: { pushes: 0, behind: 0, additions: 0, deletions: 0 } };
    const visible = { ...base, id: "visible", name: "User work", entity_id: "visible-run" };
    const muted = { ...base, id: "muted", name: "Agent work", entity_id: "muted-run" };
    const visibleRun = { kind: "branch", run_id: "visible-run", project_id: "review-project",
      workspace_id: "visible", agents: [{ id: "visible-agent", watched: true }] };
    const mutedRun = { kind: "branch", run_id: "muted-run", project_id: "review-project",
      workspace_id: "muted", agents: [{ id: "muted-agent", watched: false }] };
    const snapshot = liveFeedSnapshot({ items: [visibleRun], runs: [visibleRun, mutedRun] },
      { projects: [{ id: "review-project", name: "Review" }] }, { workspaces: [visible, muted] }, "review-device");
    await writeCached({ deviceId: "review-device", entityId: "", kind: "projects" }, snapshot.projects);
    await writeCached({ deviceId: "review-device", entityId: "", kind: "workspaces" }, snapshot.workspaces);
    await writeCached({ deviceId: "review-device", entityId: "", kind: "feed" }, snapshot);
    // This simulates the state push that used to leak a muted board row.
    await writeCached({ deviceId: "review-device", entityId: "muted-run", kind: "row" }, stampRow(mutedRun, "review-device"));
    window.__layoutModules.contexts.adoptDeviceSession({ deviceId: "review-device", call: async () => ({}),
      close() {}, peer() {}, onCarrier() {}, onPush() {} });
    await initInboxRail();
    await startFeed();
  });
  await page.waitForFunction(() => [...document.querySelectorAll("#inbox-list .inbox-entry")]
    .map((entry) => entry.dataset.key).join("|") ===
    "workspace:review-device/visible|project-agent:review-device/review-project");
  const rows = await page.locator("#inbox-list .inbox-entry").evaluateAll((entries) =>
    entries.map((entry) => ({ key: entry.dataset.key, text: entry.textContent })));
  if (rows.length !== 2 || rows[0]?.key !== "workspace:review-device/visible" || !rows[0].text.includes("User work")
    || rows[1]?.key !== "project-agent:review-device/review-project" || rows.some((row) => row.text.includes("Agent work"))) {
    throw new Error(`Unexpected inbox rows: ${JSON.stringify(rows)}`);
  }
  await page.locator("#inbox-rail").screenshot({ path: join(output, "inbox-muted.png"), animations: "disabled" });
}, { width: 1280, height: 430 });

await withLayoutPage(async ({ page, basePath }) => {
  await mountLayout(page, `<div id="shell"><aside id="inbox-rail"></aside><div id="view">
    <header id="toolbar">Build / Agent workspace</header><div id="view-body">
    <main id="root"><h1>Agent workspace</h1></main>
    <aside id="agent-rail" aria-label="Agents"></aside></div></div></div>`, { basePath });
  await loadBrowserModules(page, {
    rail: "src/core/agentRail.js",
    cache: "src/core/localCache.js",
    changes: "src/core/changeEvents.js",
  }, basePath);
  await page.evaluate(async () => {
    const { mountAgentRail } = window.__layoutModules.rail;
    const { writeCached } = window.__layoutModules.cache;
    const { greetBridge } = window.__layoutModules.changes;
    await greetBridge(async () => ({ push_events: true, api_version: "2.0.0", capabilities: ["changes.subscriptions", "requests.priority", "errors.codes", "diffs.perFile", "tasks.context", "tasks.attachments", "tasks.watching", "conversations.settings"] }), { deviceId: "review-device" });
    await writeCached({ deviceId: "review-device", entityId: "review-run", kind: "row" }, {
      run_id: "review-run", project_id: "review-project", agents: [{
        id: "review-agent", ordinal: 1, provider: "claude_adk", state: "live",
        name: "Agent-created agent", topic: "Working", watched: false,
        working: true, unread_count: 0,
      }],
    });
    mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "project", deviceId: "review-device", projectId: "review-project",
      entityId: "review-run", openAgentId: "review-agent", panelOpen: true,
      call: async (method) => method === "models.list"
        ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
        : { items: [] },
    });
  });
  const watch = page.locator('#agent-rail .rail-watch[aria-pressed="false"]');
  await watch.waitFor();
  if (await watch.getAttribute("title") !== "Not watching") throw new Error("Review agent's watch control is mislabeled");
  await watch.hover();
  await page.locator("#agent-rail .rail-head").screenshot({ path: join(output, "agent-unwatched.png"), animations: "disabled" });
  await page.locator(".rail-overview-toggle").click();
  const overview = page.locator('#agent-rail .rail-overview-row[data-overview-agent="review-agent"]');
  await overview.locator('.rail-overview-watch[aria-label="Not watching"]').waitFor();
  await overview.screenshot({ path: join(output, "agent-overview-unwatched.png"), animations: "disabled" });
}, { width: 1320, height: 850 });
