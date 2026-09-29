// #216 in a real browser: an agent's Agents panel lists its harness sub-agents
// and, apart from them, the Build agents it made; a press on a Build agent
// opens that agent's chat; and the creator's bubble wears the working dot
// while its own loop waits and its Build agent runs. Phone and desktop.
import assert from "node:assert/strict";
import { it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// The inbox is a drawer on a phone, shut until it is asked for.
const markup = (desktop) => `<div id="shell">${desktop ? '<aside id="inbox-rail"></aside>' : ""}<div id="view">
  <header id="toolbar">Build / Project</header>
  <div id="view-body"><nav id="dir-rail"></nav>
    <main id="root"><h1>Project</h1></main>
    <aside id="agent-rail" aria-label="Agents"></aside>
  </div><div id="console-region"></div>
</div></div>`;

const VIEWPORTS = [
  ["phone", { width: 390, height: 844 }],
  ["desktop", { width: 1440, height: 900 }],
];

async function mountSeededRail(page, basePath) {
  await loadBrowserModules(page, { rail: "src/core/agentRail.js", cache: "src/core/localCache.js" }, basePath);
  await page.evaluate(async () => {
    const { mountAgentRail } = window.__layoutModules.rail;
    const { writeCached } = window.__layoutModules.cache;
    const device = "layout-device";
    const since = new Date(Date.now() - 95_000).toISOString();
    await writeCached({ deviceId: device, entityId: "layout-project-run", kind: "row", sub: "" }, {
      kind: "project", run_id: "layout-project-run", project_id: "layout-project", agents: [{
        id: "boss", ordinal: 1, provider: "claude_adk", state: "live", name: "Orchestrator", topic: "Rolling the batch",
        working: false, unread_count: 0,
        surfaces: { subagents: [{ id: "sub-1", label: "Explore the rail", state: "running", started_at: Date.now() - 30_000 }] },
      }],
    });
    await writeCached({ deviceId: device, entityId: "layout-ws-run", kind: "row", sub: "" }, {
      kind: "workspace", run_id: "layout-ws-run", workspace_id: "layout-ws", title: "Fix the login redirect",
      project_id: "layout-project", agents: [
        { id: "worker", ordinal: 1, provider: "claude_adk", state: "live", name: "Login fixer", created_by: "boss",
          working: true, working_time: { since }, unread_count: 0 },
        { id: "helper", ordinal: 2, provider: "claude_adk", state: "idle", name: "Docs helper", created_by: "boss",
          working: false, unread_count: 0 },
        { id: "bystander", ordinal: 3, provider: "claude_adk", state: "idle", name: "Not mine", working: false, unread_count: 0 },
      ],
    });
    localStorage.setItem("build.rail.expanded", "1");
    window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "project", deviceId: device, projectId: "layout-project", entityId: "layout-project-run",
      call: async (method) => method === "models.list"
        ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
        : { items: [] },
    });
  });
}

const inViewport = (box, viewport) => box && box.x >= -1 && box.y >= -1
  && box.x + box.width <= viewport.width + 1 && box.y + box.height <= viewport.height + 1;

for (const [name, viewport] of VIEWPORTS) {
  it(`groups sub-agents and Build agents, and opens a Build agent's chat (${name})`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, markup(viewport.width > 760), { basePath, styles: "#toolbar{padding:12px 20px} #root{padding:24px}" });
      await mountSeededRail(page, basePath);

      // The creator's own loop waits; its Build agent works, so its bubble does.
      await page.waitForSelector(".rail-strip .working", { timeout: 5000 });

      // A phone keeps the chat shut until the reader opens it from its bubble.
      if (!(await page.locator("#rail-panel:not([aria-hidden='true'])").count())) {
        await page.locator(".rail-strip .working").first().click();
      }
      const pill = page.locator('[data-surface-kind="subagents"]');
      await pill.waitFor({ timeout: 5000 });
      assert.equal(await pill.locator(".surface-pill-count").textContent(), "2");
      await pill.click();
      await page.waitForSelector('.surface-group[data-group="build_agents"]:not([hidden])', { timeout: 5000 });
      await page.waitForFunction(() => !document.querySelector("#rail-panel")?.getAnimations({ subtree: true })
        .some((animation) => animation.effect?.getTiming().iterations !== Infinity), null, { timeout: 5000 });

      const read = await page.evaluate(() => {
        const group = (key) => document.querySelector(`.surface-group[data-group="${key}"]`);
        const box = (element) => {
          if (!element) return null;
          const { x, y, width, height } = element.getBoundingClientRect();
          return { x, y, width, height };
        };
        return {
          heads: [...document.querySelectorAll(".surface-group:not([hidden]) .surface-group-head")].map((head) => head.textContent.trim()),
          subagents: [...group("subagents").querySelectorAll(".surface-row-label")].map((label) => label.textContent),
          builds: [...group("build_agents").querySelectorAll(".surface-build-agent")].map((row) => row.dataset.buildAgent),
          firstBuild: box(group("build_agents").querySelector(".surface-build-agent")),
          overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });
      assert.deepEqual(read.heads, ["Sub-agents", "Build agents"]);
      assert.deepEqual(read.subagents, ["Explore the rail"]);
      // Only what this agent made, running first; the bystander is nobody's.
      assert.deepEqual(read.builds, ["worker", "helper"]);
      assert.ok(inViewport(read.firstBuild, viewport), `the first Build agent is on screen: ${JSON.stringify(read.firstBuild)}`);
      assert.ok(read.overflow <= 0, `nothing scrolls sideways (${read.overflow}px)`);
      await captureLayout(page, `build-agents-panel-${name}.png`);

      await page.locator('.surface-build-agent[data-build-agent="worker"]').click();
      await page.waitForFunction(() => location.hash.includes("layout-ws"), null, { timeout: 5000 });
      const hash = await page.evaluate(() => decodeURIComponent(location.hash));
      assert.ok(hash.includes("layout-ws") && hash.includes("worker"), `the press routes to the worker's chat: ${hash}`);
    }, viewport);
  });
}
