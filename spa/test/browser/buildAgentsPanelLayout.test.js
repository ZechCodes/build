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
  ["narrow phone", { width: 320, height: 640 }],
  ["phone", { width: 390, height: 844 }],
  ["desktop", { width: 1440, height: 900 }],
];

async function mountSeededRail(page, basePath) {
  await loadBrowserModules(page, {
    rail: "src/core/agentRail.js", cache: "src/core/localCache.js", lineageSupport: "src/core/agentLineageSupport.js",
  }, basePath);
  await page.evaluate(async ({ workspaceTitle, RAW_MODEL_ID }) => {
    const { mountAgentRail } = window.__layoutModules.rail;
    const { writeCached } = window.__layoutModules.cache;
    const device = "layout-device";
    // A bridge that announces agents.createdBy greeted this machine (#221).
    await window.__layoutModules.lineageSupport.rememberAgentLineageSupport(device, { agents: { createdBy: true } });
    const since = new Date(Date.now() - 95_000).toISOString();
    await writeCached({ deviceId: device, entityId: "layout-project-run", kind: "row", sub: "" }, {
      kind: "project", run_id: "layout-project-run", project_id: "layout-project", agents: [{
        id: "boss", ordinal: 1, provider: "claude_adk", state: "live", name: "Orchestrator", topic: "Rolling the batch",
        working: false, unread_count: 0,
        surfaces: { subagents: [{ id: "sub-1", label: "Explore the rail", model: RAW_MODEL_ID,
          state: "running", started_at: Date.now() - 30_000 }] },
      }],
    });
    await writeCached({ deviceId: device, entityId: "layout-ws-run", kind: "row", sub: "" }, {
      kind: "workspace", run_id: "layout-ws-run", workspace_id: "layout-ws", title: workspaceTitle,
      project_id: "layout-project", agents: [
        // Long names, so the name is what gives way; the models are short (#257).
        { id: "worker", ordinal: 1, provider: "claude_adk", state: "live", name: "Divider for markdown horizontal rules", created_by: "boss",
          model: "claude-opus-5-5", active_model: "claude-opus-5-5",
          working: true, working_time: { since }, unread_count: 0 },
        { id: "helper", ordinal: 2, provider: "codex", state: "idle", name: "Docs helper for every sign-on provider", created_by: "boss",
          model: "gpt-6-astra", working: false, unread_count: 0 },
        // A model no rule knows keeps its whole raw id, wrapping rather than
        // clipped or pushing the row past its edge (#226, #257).
        { id: "tinkerer", ordinal: 4, provider: "claude_adk", state: "live", name: "Model tinkerer", created_by: "boss",
          model: RAW_MODEL_ID, working: true, working_time: { since }, unread_count: 0 },
        { id: "bystander", ordinal: 3, provider: "claude_adk", state: "idle", name: "Not mine", working: false, unread_count: 0 },
      ],
    });
    localStorage.setItem("build.rail.expanded", "1");
    window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "project", deviceId: device, projectId: "layout-project", entityId: "layout-project-run",
      call: async (method) => method === "models.list"
        ? { default_provider: "claude_adk", providers: [
          { id: "claude_adk", label: "Claude Code", models: [{ id: "claude-opus-5-5", label: "Claude Opus 5.5" }], efforts: [] },
          { id: "codex", label: "Codex", models: [{ id: "gpt-6-astra", label: "GPT-6-Astra" }], efforts: [] },
        ] }
        : { items: [] },
    });
  }, { workspaceTitle: WORKSPACE_TITLE, RAW_MODEL_ID });
}

const RAW_MODEL_ID = "house-research-model-20260915-experimental-preview";
const WORKSPACE_TITLE = "Review #253: markdown dividers in every theme";

/** The narrowest the agent's name gets on a Build agent or sub-agent row:
 *  enough to read a short name whole and a long one's start (#226, #261). */
const MIN_NAME_WIDTH = 64;

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
      assert.equal(await pill.locator(".surface-pill-count").textContent(), "3");
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
        const subagent = group("subagents").querySelector(".surface-agent-summary");
        const subagentModel = subagent.querySelector(".surface-row-model");
        return {
          heads: [...document.querySelectorAll(".surface-group:not([hidden]) .surface-group-head")].map((head) => head.textContent.trim()),
          subagents: [...group("subagents").querySelectorAll(".surface-row-label")].map((label) => label.textContent),
          subagentRow: {
            row: box(subagent),
            name: box(subagent.querySelector(".surface-row-label")),
            model: subagentModel.textContent,
            modelBox: box(subagentModel),
            modelFits: { scrollWidth: subagentModel.scrollWidth, clientWidth: subagentModel.clientWidth },
          },
          builds: [...group("build_agents").querySelectorAll(".surface-build-agent")].map((row) => row.dataset.buildAgent),
          firstBuild: box(group("build_agents").querySelector(".surface-build-agent")),
          buildRows: [...group("build_agents").querySelectorAll(".surface-build-agent")].map((row) => ({
            row: box(row),
            name: box(row.querySelector(".surface-row-label")),
            head: box(row.querySelector(".surface-row-head")),
            parts: [...row.querySelectorAll(".surface-row-head > *")].map(box),
            text: row.textContent,
            model: row.querySelector(".surface-row-model")?.textContent || "",
            modelFits: (({ scrollWidth, clientWidth }) => ({ scrollWidth, clientWidth }))(row.querySelector(".surface-row-model")),
            modelBox: box(row.querySelector(".surface-row-model")),
            modelLineHeight: parseFloat(getComputedStyle(row.querySelector(".surface-row-model")).lineHeight),
            clock: box(row.querySelector(".surface-row-clock")),
          })),
          overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });
      assert.deepEqual(read.heads, ["Sub-agents", "Build agents"]);
      assert.deepEqual(read.subagents, ["Explore the rail"]);
      // Only what this agent made, running first; the bystander is nobody's.
      assert.deepEqual(read.builds, ["worker", "tinkerer", "helper"]);
      for (const theme of ["light", "dark"]) {
        await page.evaluate((value) => { document.documentElement.dataset.theme = value; }, theme);
        await captureLayout(page, `build-agents-panel-${name}-${theme}.png`);
      }
      await page.evaluate(() => { delete document.documentElement.dataset.theme; });
      assert.ok(inViewport(read.firstBuild, viewport), `the first Build agent is on screen: ${JSON.stringify(read.firstBuild)}`);
      assert.ok(read.overflow <= 0, `nothing scrolls sideways (${read.overflow}px)`);
      // #261: a raw model wraps without squeezing the sub-agent's name away.
      const { row: subagent, name: subagentName, model, modelBox, modelFits } = read.subagentRow;
      assert.equal(model, RAW_MODEL_ID);
      assert.ok(subagentName.width >= MIN_NAME_WIDTH, `the sub-agent name keeps a readable width: ${subagentName.width}px`);
      assert.ok(modelFits.scrollWidth <= modelFits.clientWidth, `the sub-agent model is not clipped: ${JSON.stringify(modelFits)}`);
      assert.ok(modelBox.x >= subagent.x && modelBox.x + modelBox.width <= subagent.x + subagent.width + 1
        && modelBox.y >= subagent.y && modelBox.y + modelBox.height <= subagent.y + subagent.height + 1,
      `the sub-agent model stays inside its row: ${JSON.stringify({ modelBox, subagent })}`);
      // #257: each row wears its model's short name, whole, and no longer
      // names its workspace; the agent's name is what gives way (#226).
      assert.deepEqual(read.buildRows.map((row) => row.model), ["Opus 5.5", RAW_MODEL_ID, "6 Astra"]);
      // A short name stays on one line; only the raw id wraps.
      for (const { modelBox, modelLineHeight } of [read.buildRows[0], read.buildRows[2]]) {
        assert.ok(modelBox.height < modelLineHeight * 1.5, `a short name keeps one line: ${JSON.stringify(modelBox)}`);
      }
      for (const { row, head, name: label, parts, text, modelFits, modelBox, clock } of read.buildRows) {
        assert.ok(modelFits.scrollWidth <= modelFits.clientWidth, `the model is not clipped: ${JSON.stringify(modelFits)}`);
        // The row does not clip it either: the model ends inside the row.
        assert.ok(modelBox.x + modelBox.width <= row.x + row.width + 1, `the model ends inside its row: ${JSON.stringify({ modelBox, row })}`);
        assert.ok(clock || text.includes("6 Astra"), `a working row shows its clock: ${text}`);
        if (clock) {
          assert.ok(clock.x + clock.width <= row.x + row.width + 1 && inViewport(clock, viewport),
            `the clock stays inside its row: ${JSON.stringify({ clock, row })}`);
        }
        assert.ok(!text.includes(WORKSPACE_TITLE), `the row leaves its workspace to the tooltip: ${text}`);
        assert.ok(label.width >= MIN_NAME_WIDTH, `the name keeps a readable width: ${label.width}px`);
        // The name takes up the slack, so the row's last part (the clock, on a
        // running row) ends at the row's right edge, in line with the rows above.
        const last = parts.filter((part) => part.width > 0).at(-1);
        assert.ok(Math.abs(last.x + last.width - (head.x + head.width)) <= 1,
          `the row's last part ends at its right edge: ${JSON.stringify({ last, head })}`);
        for (const part of parts) {
          if (part.width > 0) assert.ok(part.x + part.width <= row.x + row.width + 1, `a row part runs past the row: ${JSON.stringify({ part, row })}`);
        }
      }
      await page.locator('.surface-build-agent[data-build-agent="worker"]').click();
      await page.waitForFunction(() => location.hash.includes("layout-ws"), null, { timeout: 5000 });
      const hash = await page.evaluate(() => decodeURIComponent(location.hash));
      assert.ok(hash.includes("layout-ws") && hash.includes("worker"), `the press routes to the worker's chat: ${hash}`);
    }, viewport);
  });
}
