// Capture the two review images for #101 with the production inbox and rail.
// Run from spa/: node test/browser/captureWatching.mjs
import { mkdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const output = fileURLToPath(new URL("../../../design/watching-defaults/", import.meta.url));
const html = await readFile(fileURLToPath(new URL("../../index.html", import.meta.url)), "utf8");
const body = html.match(/<body>([\s\S]*)<\/body>/)[1];
await mkdir(output, { recursive: true });

await withLayoutPage(async ({ page, basePath }) => {
  await mountLayout(page, body, { basePath });
  await loadBrowserModules(page, {
    app: "src/app.js",
    cache: "src/core/localCache.js",
    feed: "src/core/taskFeed.js",
    inbox: "src/core/inboxShell.js",
  }, basePath);
  await page.evaluate(async () => {
    const { App } = window.__layoutModules.app;
    const { writeCached } = window.__layoutModules.cache;
    const { startFeed } = window.__layoutModules.feed;
    const { initInboxRail } = window.__layoutModules.inbox;
    App.route = { name: "inbox" };
    App.devices = [{ id: "review-device", name: "This computer", status: "online" }];
    App.selectedDeviceId = "review-device";
    const base = { project_id: "review-project", status: "ready", managed: true,
      directories: [{ id: "src", source_id: "review-source", is_git: true }],
      work_summary: { pushes: 0, behind: 0, additions: 0, deletions: 0 } };
    const visible = { ...base, id: "visible", name: "User work", entity_id: "visible-run" };
    const muted = { ...base, id: "muted", name: "Agent work", entity_id: "muted-run" };
    const visibleRun = { kind: "branch", run_id: "visible-run", project_id: "review-project",
      workspace_id: "visible", agents: [{ id: "visible-agent", watched: true }] };
    const mutedRun = { kind: "branch", run_id: "muted-run", project_id: "review-project",
      workspace_id: "muted", agents: [{ id: "muted-agent", watched: false }] };
    await writeCached({ deviceId: "review-device", entityId: "", kind: "projects" },
      [{ id: "review-project", name: "Review" }]);
    await writeCached({ deviceId: "review-device", entityId: "", kind: "workspaces" }, [visible, muted]);
    await writeCached({ deviceId: "review-device", entityId: "", kind: "feed" },
      { items: [visibleRun], runs: [visibleRun, mutedRun] });
    // This simulates the state push that used to leak a muted board row.
    await writeCached({ deviceId: "review-device", entityId: "muted-run", kind: "row" }, mutedRun);
    await initInboxRail();
    await startFeed();
  });
  await page.waitForFunction(() => document.querySelectorAll("#inbox-list .inbox-entry").length === 1);
  const names = await page.locator("#inbox-list .inbox-entry").allTextContents();
  if (!names[0]?.includes("User work") || names.some((name) => name.includes("Agent work"))) {
    throw new Error(`Unexpected inbox rows: ${names.join(", ")}`);
  }
  await page.locator("#inbox-rail").screenshot({ path: `${output}inbox-muted.png` });
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
    await greetBridge(async () => ({ push_events: true, api_version: "1.13.0" }), { deviceId: "review-device" });
    await writeCached({ deviceId: "review-device", entityId: "review-run", kind: "row" }, {
      run_id: "review-run", project_id: "review-project", agents: [{
        id: "review-agent", ordinal: 1, provider: "claude_adk", state: "live",
        name: "Agent-created agent", topic: "Working", watched: false,
        working: true, unread_count: 0,
      }],
    });
    localStorage.setItem("build.rail.expanded", "1");
    mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "project", deviceId: "review-device", projectId: "review-project",
      entityId: "review-run", call: async (method) => method === "models.list"
        ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
        : { items: [] },
    });
  });
  await page.waitForSelector(".rail-watch");
  await page.waitForFunction(() => document.querySelector(".rail-watch")?.getAttribute("aria-pressed") === "false");
  await page.locator(".rail-watch").hover();
  await page.locator(".rail-head").screenshot({ path: `${output}agent-unwatched.png` });
  await page.locator(".rail-expand").click();
  await page.waitForFunction(() => document.querySelector(".rail-overview-row")?.textContent.includes("Not watching"));
  await page.locator(".rail-overview-row").screenshot({ path: `${output}agent-overview-unwatched.png` });
}, { width: 1320, height: 850 });
