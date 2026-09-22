import assert from "node:assert/strict";
import { it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const markup = `<div id="shell"><aside id="inbox-rail"></aside><div id="view">
  <header id="toolbar">Build / Workspace</header>
  <div id="view-body"><nav id="dir-rail"></nav>
    <main id="root"><h1>Workspace</h1><button id="workspace-action">Review changes</button></main>
    <aside id="agent-rail" aria-label="Agents"></aside>
  </div><div id="console-region"></div>
</div></div>`;

const rects = () => {
  const rect = (selector) => {
    const { x, y, width, height } = document.querySelector(selector).getBoundingClientRect();
    return { x, y, width, height };
  };
  const work = rect("#root");
  const point = { x: work.x + 35, y: work.y + 80 };
  return {
    work, rail: rect("#agent-rail"), strip: rect(".rail-strip"),
    panel: document.querySelector(".rail-panel") ? rect(".rail-panel") : null,
    overview: document.querySelector(".rail-overview-content") ? rect(".rail-overview-content") : null,
    workHit: document.elementFromPoint(point.x, point.y)?.closest("#root")?.id || null,
  };
};

const sameRect = (actual, expected) => {
  for (const key of ["x", "y", "width", "height"]) {
    assert.ok(Math.abs(actual[key] - expected[key]) <= 1,
      `${key}: ${actual[key]} differs from ${expected[key]}`);
  }
};

it("mounted agent overview replaces only the chat panel in docked and popover modes", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, markup, {
      basePath,
      styles: "#toolbar{padding:12px 20px} #root{padding:24px} #root h1{font-size:20px}",
    });
    await loadBrowserModules(page, {
      rail: "src/core/agentRail.js",
      cache: "src/core/localCache.js",
    }, basePath);
    await page.evaluate(async () => {
      const { mountAgentRail } = window.__layoutModules.rail;
      const { writeCached } = window.__layoutModules.cache;
      await writeCached({ deviceId: "layout-device", entityId: "layout-run", kind: "row", sub: "" }, {
        run_id: "layout-run", project_id: "layout-project", agents: [{
          id: "layout-agent", ordinal: 1, provider: "claude_adk", state: "live",
          name: "Layout agent", topic: "Checking overview bounds", working: true, unread_count: 0,
        }],
      });
      localStorage.setItem("build.rail.expanded", "1");
      window.__workspaceClicks = 0;
      document.querySelector("#workspace-action").addEventListener("click", () => { window.__workspaceClicks += 1; });
      window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), {
        kind: "project", deviceId: "layout-device", projectId: "layout-project",
        entityId: "layout-run", call: async (method) => method === "models.list"
          ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
          : { items: [] },
      });
    });

    await page.waitForSelector(".rail-panel:not([aria-hidden='true'])");
    await page.waitForSelector(".rail-expand");
    const dockedPanel = await page.evaluate(rects);
    assert.ok(dockedPanel.panel.width > 0);
    await page.locator(".rail-expand").click();
    await page.waitForSelector(".rail-overview-content");
    const dockedOverview = await page.evaluate(rects);
    sameRect(dockedOverview.overview, dockedPanel.panel);
    sameRect(dockedOverview.strip, dockedPanel.strip);
    assert.equal(dockedOverview.workHit, "root");
    assert.ok(dockedOverview.work.x + dockedOverview.work.width <= dockedOverview.overview.x,
      "the docked overview must start to the right of the workspace");
    await page.locator("#workspace-action").click();
    assert.equal(await page.evaluate(() => window.__workspaceClicks), 1);

    await page.locator(".rail-expand").click();
    await page.waitForSelector(".rail-panel:not([aria-hidden='true'])");
    await page.locator(".rail-panel .pinbtn").click();
    await page.waitForSelector("#agent-rail.rail-popover:not([data-panel-transition])");
    const popoverPanel = await page.evaluate(rects);
    assert.equal(Math.round(popoverPanel.rail.width), Math.round(popoverPanel.strip.width));
    await page.locator(".rail-expand").click();
    await page.waitForSelector(".rail-overview-content");
    const popoverOverview = await page.evaluate(rects);
    sameRect(popoverOverview.overview, popoverPanel.panel);
    sameRect(popoverOverview.strip, popoverPanel.strip);
    assert.equal(popoverOverview.workHit, "root");
    assert.equal(Math.round(popoverOverview.rail.width), Math.round(popoverPanel.rail.width));
    await page.locator("#workspace-action").click();
    assert.equal(await page.evaluate(() => window.__workspaceClicks), 2);
  }, { width: 1320, height: 850 });
}, 30_000);
