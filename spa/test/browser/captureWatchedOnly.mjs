// Capture the review images for #105 with the production rail: the strip with
// only watched agents, the overview's not-watching marks (#186), and an unwatched
// agent's temporary bubble before and after the reader leaves it.
// Run from spa/: node test/browser/captureWatchedOnly.mjs
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const output = fileURLToPath(new URL("../../../design/rail-watched-only/", import.meta.url));
await mkdir(output, { recursive: true });

await withLayoutPage(async ({ page, basePath }) => {
  await mountLayout(page, `<div id="shell"><aside id="inbox-rail"></aside><div id="view">
    <header id="toolbar">Build / Rail work</header><div id="view-body">
    <main id="root"><h1>Rail work</h1></main>
    <aside id="agent-rail" aria-label="Agents"></aside></div></div></div>`, { basePath });
  await loadBrowserModules(page, {
    rail: "src/core/agentRail.js",
    changes: "src/core/changeEvents.js",
    fixture: "test/railCacheFixture.js",
    app: "src/app.js",
    cache: "src/core/localCache.js",
    feed: "src/core/taskFeed.js",
    feedMerge: "src/core/feedMerge.js",
  }, basePath);
  await page.evaluate(async () => {
    const { mountAgentRail } = window.__layoutModules.rail;
    const { greetBridge } = window.__layoutModules.changes;
    const { writeRailBoard, writeRailThread } = window.__layoutModules.fixture;
    await greetBridge(async () => ({ push_events: true, api_version: "2.0.0", capabilities: ["tasks.watching"] }), { deviceId: "dev-1" });
    const agent = (id, ordinal, name, watched, over = {}) => ({ id, ordinal, name, topic: name, watched,
      provider: "claude_adk", state: "live", working: false, unread_count: 0, ...over });
    const said = (sequence, body, day) => ({ type: "message",
      data: { sequence, role: "agent", body, created_at: `2026-09-${day}T12:00:00Z` } });
    const rail = { kind: "workspace", workspace_id: "ws-rail", entity_id: "run-rail", project_id: "p1", agents: [
      agent("ag-rail", 1, "Rail watched only", true, { working: true }),
      agent("ag-review", 2, "Review helper", false),
      agent("ag-tests", 3, "Test runner", false),
    ] };
    const docs = { kind: "workspace", workspace_id: "ws-docs", entity_id: "run-docs", project_id: "p1", agents: [
      agent("ag-docs", 1, "Docs writer", true),
      agent("ag-links", 2, "Link checker", false),
    ] };
    const project = { kind: "project", project_id: "p1", entity_id: "run-project", agents: [
      agent("ag-project", 1, "Project agent", true),
    ] };
    await writeRailBoard({
      projects: [{ project_id: "p1", name: "Build", entity_id: "run-project" }],
      workspaces: [
        { id: "ws-rail", project_id: "p1", name: "Rail work", entity_id: "run-rail" },
        { id: "ws-docs", project_id: "p1", name: "Docs pass", entity_id: "run-docs" },
      ],
      items: [rail, docs, project],
    });
    // The board as the sync layer holds it, so the rail's feed names the
    // workspaces its overview heads.
    const { App } = window.__layoutModules.app;
    const { writeCached } = window.__layoutModules.cache;
    const { liveFeedSnapshot } = window.__layoutModules.feedMerge;
    App.devices = [{ id: "dev-1", name: "This computer", status: "online" }];
    App.selectedDeviceId = "dev-1";
    const view = liveFeedSnapshot({ items: [rail, docs, project], runs: [] },
      { projects: [{ project_id: "p1", name: "Build", entity_id: "run-project" }] },
      { workspaces: [
        { id: "ws-rail", workspace_id: "ws-rail", project_id: "p1", name: "Rail work", entity_id: "run-rail" },
        { id: "ws-docs", workspace_id: "ws-docs", project_id: "p1", name: "Docs pass", entity_id: "run-docs" },
      ] }, "dev-1");
    await writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, view);
    await window.__layoutModules.feed.startFeed();
    await writeRailThread("run-rail", "ag-rail", { items: [said(1, "The strip now filters on watched.", 26)] });
    await writeRailThread("run-rail", "ag-review", { items: [said(1, "Two findings on the overview markup.", 25)] });
    await writeRailThread("run-rail", "ag-tests", { items: [said(1, "Suite is green on the branch.", 24)] });
    await writeRailThread("run-docs", "ag-docs", { items: [said(1, "Drafted the rail section.", 23)] });
    await writeRailThread("run-docs", "ag-links", { items: [said(1, "No broken links.", 22)] });
    await writeRailThread("run-project", "ag-project", { items: [said(1, "Assigned #105.", 26)] });
    localStorage.setItem("build.rail.expanded", "1");
    mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: "ws-rail",
      projectAgent: { projectId: "p1", entityId: "run-project", name: "Build" },
      call: async (method) => method === "models.list"
        ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
        : { items: [] },
    });
  });

  const stripIds = () => page.evaluate(() => [...document.querySelectorAll('.rail-bubble[data-bubble="agent"]')]
    .map((node) => node.dataset.agent).join(","));
  const waitForStrip = (ids) => page.waitForFunction((want) => [...document.querySelectorAll('.rail-bubble[data-bubble="agent"]')]
    .map((node) => node.dataset.agent).join(",") === want, ids);

  await waitForStrip("ag-rail");
  await page.waitForFunction(() => document.querySelector('.rail-bubble[data-agent="ag-rail"]')?.classList.contains("active")
    && document.querySelector(".rail-who")?.title === "Rail watched only");
  await page.locator("#agent-rail").screenshot({ path: `${output}rail-watched-only.png` });

  await page.locator(".rail-overview-toggle").click();
  const notWatching = (count) => page.waitForFunction((want) =>
    document.querySelectorAll('.rail-overview-section .rail-overview-row-unwatched').length === want, count);
  // The workspace page's overview opens on its own workspace, beside the
  // project's agents; All workspaces widens it to every workspace.
  await notWatching(2);
  await page.locator("#agent-rail").screenshot({ path: `${output}overview-not-watching.png` });
  await page.locator(".rail-overview-up").click();
  await notWatching(3);
  await page.locator("#agent-rail").screenshot({ path: `${output}overview-not-watching-all-workspaces.png` });

  await page.locator('.rail-overview-row-unwatched[data-overview-agent="ag-review"]').click();
  await waitForStrip("ag-rail,ag-review");
  await page.waitForFunction(() => document.querySelector('.rail-bubble[data-agent="ag-review"]')?.classList.contains("active")
    && document.querySelector(".rail-who")?.title === "Review helper");
  await page.locator("#agent-rail").screenshot({ path: `${output}temporary-bubble.png` });

  await page.locator('.rail-bubble[data-agent="ag-rail"]').click();
  await waitForStrip("ag-rail");
  await page.waitForFunction(() => document.querySelector(".rail-who")?.title === "Rail watched only");
  await page.locator("#agent-rail").screenshot({ path: `${output}temporary-bubble-gone.png` });
  console.log(`strip after leaving: ${await stripIds()}`);
}, { width: 1320, height: 850 });
