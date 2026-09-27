// Capture the workspace headings' right cluster (#192) with the production
// rail on the Skrift picture (test/agentsOverviewFixture.js): a phone at
// 390x844, dark and light, pinned open the way the maintainer's screenshot had
// it. The fixture is bent in one place so the three states are all on the one
// screen: skrift-fixes keeps only its working agent and gives it an unread, so
// its heading is pill + working; skrift-review is pill + idle;
// issue-implementation-audit is no pill + idle. Run from spa/:
//   node test/browser/captureHeadingSlots.mjs [label] [output directory]
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { homedir } from "node:os";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const label = process.argv[2] || "after";
const output = process.argv[3] || join(homedir(), ".build/project-scratch/heading-slots-shots");
await mkdir(output, { recursive: true });

const shellHtml = `<div id="shell"><div id="view">
  <header id="toolbar">Skrift · Tasks</header>
  <div id="view-body"><nav id="dir-rail"></nav>
    <main id="root"><h1>Tasks</h1><p>Board</p></main>
    <aside id="agent-rail" aria-label="Agents"></aside>
  </div><div id="console-region"></div>
</div></div>`;
const styles = "#toolbar{padding:12px 20px} #root{padding:24px} #root h1{font-size:20px;margin:0 0 8px}";

// A working agent's pulse runs for as long as it works; everything else settles.
const settled = (page) => page.waitForFunction(() => document.getAnimations()
  .every((animation) => animation.effect?.getTiming().iterations === Infinity), null, { timeout: 5000 });

async function seed(page, basePath) {
  await loadBrowserModules(page, { app: "src/app.js" }, basePath);
  await page.evaluate(() => { window.__appModule = window.__layoutModules.app; delete window.__layoutModules; });
  await loadBrowserModules(page, {
    rail: "src/core/agentRail.js",
    cache: "src/core/localCache.js",
    merge: "src/core/feedMerge.js",
    tracker: "src/core/trackerCache.js",
    fixture: "test/agentsOverviewFixture.js",
  }, basePath);
  await page.evaluate(async () => {
    const { mountAgentRail } = window.__layoutModules.rail;
    const { writeCached } = window.__layoutModules.cache;
    const { stampWorkspace } = window.__layoutModules.merge;
    const { writeTasksRecord } = window.__layoutModules.tracker;
    const { writeAgentsOverviewFixture, overviewRailContext, OVERVIEW_ROWS, OVERVIEW_DEVICE, OVERVIEW_PROJECT } = window.__layoutModules.fixture;
    await writeAgentsOverviewFixture({ writeCached, stampWorkspace, writeTasksRecord });
    // skrift-fixes: the working agent alone, with an unread, for pill + working.
    const fixes = OVERVIEW_ROWS.find((row) => row.workspaceId === "ws-fixes");
    const fixer = fixes.agents.find((one) => one.id === "fixer");
    await writeCached({ deviceId: OVERVIEW_DEVICE, entityId: fixes.entityId, kind: "row", sub: "" }, {
      kind: "workspace", entity_id: fixes.entityId, project_id: OVERVIEW_PROJECT, workspace_id: "ws-fixes",
      agents: [{ ...fixer, unread_count: 2, unread_reason: "agent_message" }],
    });
    localStorage.setItem("build.rail.expanded", "1");
    window.__overviewRail = mountAgentRail(document.querySelector("#agent-rail"), overviewRailContext());
  });
  await page.waitForSelector(".rail-overview-toggle", { timeout: 5000 });
  await page.locator(".rail-overview-toggle").click();
  await page.waitForFunction(() => document.querySelectorAll("#rail-panel .rail-overview-section").length >= 5, null, { timeout: 5000 });
}

/** The panel pinned open, as the maintainer's phone had it. */
async function pinned(page) {
  const pressed = await page.evaluate(() => document.querySelector("#rail-panel .pinbtn")?.getAttribute("aria-pressed"));
  if (pressed !== "true") await page.locator("#rail-panel .pinbtn").click();
  await page.waitForFunction(() => {
    const panel = document.querySelector("#rail-panel");
    const view = document.querySelector("#view").getBoundingClientRect();
    return !document.querySelector("#agent-rail").classList.contains("rail-popover")
      && !panel.getAnimations({ subtree: true }).some((animation) => animation.effect?.getTiming().iterations !== Infinity)
      && panel.getBoundingClientRect().width >= view.width - 1;
  }, null, { timeout: 5000 });
  await page.waitForTimeout(300);
}

/** The right cluster of every workspace heading, measured. */
const metrics = () => {
  const box = (node) => { const { left, right } = node.getBoundingClientRect(); return [Math.round(left * 10) / 10, Math.round(right * 10) / 10]; };
  const list = document.querySelector(".rail-overview-list");
  return {
    sideways: list.scrollWidth - list.clientWidth,
    heads: [...document.querySelectorAll(".rail-overview-section-head")].map((head) => {
      const pill = head.querySelector(".rail-overview-need:not(.is-error)");
      const dot = head.querySelector(".rail-overview-live, .rail-overview-idle");
      const add = head.querySelector(".rail-overview-add");
      const range = document.createRange();
      if (add) range.selectNodeContents(add);
      return { name: head.closest("section").getAttribute("aria-label"), height: head.getBoundingClientRect().height,
        pill: pill && box(pill), dot: dot && [dot.className.replace("rail-overview-", ""), ...box(dot)],
        press: add && box(add), plus: add && box(range) };
    }),
  };
};

for (const theme of ["dark", "light"]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, shellHtml, { basePath, styles });
    await page.evaluate((wanted) => { document.documentElement.dataset.theme = wanted; }, theme);
    await seed(page, basePath);
    await pinned(page);
    await page.mouse.move(0, 0);
    await settled(page);
    await page.screenshot({ path: join(output, `${label}-phone-${theme}.png`) });
    console.log("%s %s %s", label, theme, JSON.stringify(await page.evaluate(metrics)));
  }, { width: 390, height: 844 });
}
console.log(`wrote ${output}`);
