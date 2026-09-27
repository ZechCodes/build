// Capture the review images for #117 with the production rail, overview and
// workspace Tasks tab. Run from spa/: node test/browser/captureChatOverview.mjs
import { mkdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { loadChatOverviewModules, seedChatOverview } from "./chatOverviewSeed.mjs";

const output = fileURLToPath(new URL("../../../design/chat-overview/", import.meta.url));
await mkdir(output, { recursive: true });
const icon = (name) => readFile(fileURLToPath(new URL(`../../node_modules/lucide-static/icons/${name}.svg`, import.meta.url)), "utf8");
const CANDIDATES = [
  ["messages-square", "Stacked chats (chosen)"],
  ["layout-list", "List of conversations"],
  ["gallery-vertical-end", "Stacked cards"],
];

// The inbox is a drawer on a phone, and shut until it is asked for.
const shellHtml = (label) => `<div id="shell">${label === "desktop" ? '<aside id="inbox-rail"></aside>' : ""}<div id="view">
  <header id="toolbar">Build / chat-overview-nav</header>
  <div id="view-body"><nav id="dir-rail"></nav>
    <main id="root"><h1>chat-overview-nav</h1><p>Changes</p></main>
    <aside id="agent-rail" aria-label="Agents"></aside>
  </div><div id="console-region"></div>
</div></div>`;
const styles = "#toolbar{padding:12px 20px} #root{padding:24px} #root h1{font-size:20px;margin:0 0 8px}";

const VIEWPORTS = [
  ["desktop", { width: 1320, height: 850 }],
  ["mobile", { width: 390, height: 844 }],
];

const sectionsAre = (page, names) => page.waitForFunction((wanted) => JSON.stringify([...document
  .querySelectorAll(".rail-overview-section")].map((section) => section.getAttribute("aria-label")))
  === JSON.stringify(wanted), names, { timeout: 5000 });
// A working agent's pulse runs for as long as it works; everything else settles.
const settled = (page) => page.waitForFunction(() => document.getAnimations()
  .every((animation) => animation.effect?.getTiming().iterations === Infinity), null, { timeout: 5000 });

for (const [label, viewport] of VIEWPORTS) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, shellHtml(label), { basePath, styles });
    await loadChatOverviewModules(page, basePath);
    await page.evaluate(seedChatOverview);
    await page.waitForSelector(".rail-bubble-add + .rail-overview-toggle", { timeout: 5000 });
    await settled(page);
    await page.screenshot({ path: `${output}strip-${label}.png` });
    if (label === "desktop") {
      // The candidates, one strip each, on a 3x page so the glyphs read at
      // review size without being a small capture blown up.
      const sharp = await page.context().browser().newPage({ viewport, deviceScaleFactor: 3 });
      await sharp.goto(page.url());
      await mountLayout(sharp, shellHtml(label), { basePath, styles });
      await loadChatOverviewModules(sharp, basePath);
      await sharp.evaluate(seedChatOverview);
      await sharp.waitForSelector(".rail-bubble-add + .rail-overview-toggle", { timeout: 5000 });
      await settled(sharp);
      const strips = [];
      for (const [name] of CANDIDATES) {
        const svg = await icon(name);
        const box = await sharp.evaluate((glyph) => {
          document.querySelector(".rail-overview-toggle").innerHTML = glyph;
          const { x, y, width } = document.querySelector(".rail-strip").getBoundingClientRect();
          return { x, y, width, height: 190 };
        }, svg);
        strips.push(await sharp.screenshot({ type: "png", clip: box }));
      }
      await sharp.close();
      await page.setContent(`<body style="margin:0;width:max-content;display:flex;gap:28px;padding:20px;background:#111;font:13px Inter,sans-serif;color:#ddd">
        ${strips.map((png, index) => `<figure style="margin:0;display:flex;flex-direction:column;align-items:center;gap:8px">
          <img src="data:image/png;base64,${png.toString("base64")}">
          <figcaption>${CANDIDATES[index][1]}<br><code>${CANDIDATES[index][0]}</code></figcaption></figure>`).join("")}
      </body>`);
      await page.locator("body").screenshot({ path: `${output}icon-candidates.png` });
      return;
    }
  }, viewport);

  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, shellHtml(label), { basePath, styles });
    await loadChatOverviewModules(page, basePath);
    await page.evaluate(seedChatOverview);
    await page.waitForSelector(".rail-overview-toggle", { timeout: 5000 });
    await page.locator(".rail-overview-toggle").click();
    await sectionsAre(page, ["Project agents", "chat-overview-nav"]);
    await settled(page);
    await page.screenshot({ path: `${output}overview-workspace-${label}.png` });
    await page.locator("#rail-panel .rail-overview-up").click();
    await sectionsAre(page, ["Project agents", "spa-flaky-tests", "landing-page", "chat-overview-nav",
      "relay-candidates", "review-system-plan"]);
    await settled(page);
    await page.screenshot({ path: `${output}overview-project-${label}.png` });
    // The workspaces with no agents sit last, still headed and still with a +.
    await page.locator('.rail-overview-section[aria-label="review-system-plan"]').scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${output}overview-empty-${label}.png` });
    await page.locator(".rail-overview-list").evaluate((list) => { list.scrollTop = 0; });
    await page.locator('.rail-overview-see-all[data-overview-scope="workspace-busy"]').click();
    await sectionsAre(page, ["Project agents", "spa-flaky-tests"]);
    await settled(page);
    await page.screenshot({ path: `${output}overview-see-all-${label}.png` });
  }, viewport);

  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, `<div id="shell"><div id="view"><header id="toolbar">Build / chat-overview-nav · Tasks</header>
      <div id="view-body"><main id="root" style="padding:0"></main></div></div></div>`, { basePath, styles: "#toolbar{padding:12px 20px}" });
    await loadChatOverviewModules(page, basePath, {
      tab: "src/core/workspaceTasksTab.js",
      tracker: "src/core/trackerCache.js",
      sheet: "src/styles/tasks.css",
    });
    await page.evaluate(async () => {
      const { mountWorkspaceTasksTab } = window.__layoutModules.tab;
      const { writeTasksRecord } = window.__layoutModules.tracker;
      const agentId = "agent-here";
      const task = (number, title, status) => ({
        id: `task-${number}`, project_id: "overview-project", number, title, body: "", state: "open", status,
        labels: ["frontend"], priority: "high", assignee: { kind: "agent", agent_id: agentId },
        links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
        created_by: { kind: "user" }, created_at: "2026-09-23T21:46:00Z", updated_at: "2026-09-23T21:46:00Z", closed_at: null,
      });
      const tasks = [task(117, "Chat overview: icon after the new-chat button", "in_progress"),
        task(116, "Agent-made attachments and a task lightbox", "ready")];
      const columns = [["backlog", "Backlog"], ["ready", "Ready"], ["in_progress", "In progress"],
        ["in_review", "In review"], ["done", "Done"]].map(([id, name]) => ({ id, name }));
      await writeTasksRecord("overview-device", "overview-project", { tasks, columns });
      const projectKey = "overview-device/overview-project";
      mountWorkspaceTasksTab(document.querySelector("#root"), {
        route: { name: "workspace", deviceId: "overview-device", projectId: "overview-project",
          workspaceId: "workspace-current", tab: "tasks" },
        context: { deviceId: "overview-device", rpc: async (method) => (method === "tasks.list" ? { tasks } : {}),
          modelCatalog: () => ({ providers: [] }), refreshModelCatalog: async () => ({ providers: [] }) },
        feed: () => ({
          workspaces: [{ id: "workspace-current", workspace_id: "workspace-current", name: "chat-overview-nav", projectKey, entity_id: "current-run" }],
          items: [{ kind: "branch", projectKey, run_id: "current-run", agents: [{ id: agentId, ordinal: 1 }] }],
        }),
        selection: null,
        navigate: () => {},
        sayWhichTask: () => {},
      });
    });
    await page.waitForSelector(".task-head .scope-link", { timeout: 5000 });
    await page.waitForSelector(".task-title", { timeout: 5000 });
    await page.screenshot({ path: `${output}tasks-tab-${label}.png` });
  }, viewport);
}
console.log(`wrote ${output}`);
