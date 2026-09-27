// Capture the agents overview (#186) with the production rail on the Skrift
// picture (test/agentsOverviewFixture.js): a phone at 390x844 in both themes,
// pinned open the way the maintainer's screenshot had it, and the desktop rail
// at 1440 wide. Run from spa/:
//   node test/browser/captureAgentsOverview.mjs [label] [output directory]
// `label` prefixes every file ("before" on main, "after" on the branch).
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { homedir } from "node:os";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const label = process.argv[2] || "after";
const output = process.argv[3] || join(homedir(), ".build/project-scratch/agents-overview-shots");
await mkdir(output, { recursive: true });

// The inbox is a drawer on a phone, and shut until it is asked for.
const shellHtml = (desktop) => `<div id="shell">${desktop ? '<aside id="inbox-rail"></aside>' : ""}<div id="view">
  <header id="toolbar">Skrift · Issues</header>
  <div id="view-body"><nav id="dir-rail"></nav>
    <main id="root"><h1>Issues</h1><p>Board</p></main>
    <aside id="agent-rail" aria-label="Agents"></aside>
  </div><div id="console-region"></div>
</div></div>`;
const styles = "#toolbar{padding:12px 20px} #root{padding:24px} #root h1{font-size:20px;margin:0 0 8px}";

const VIEWPORTS = [
  ["phone", { width: 390, height: 844 }],
  ["desktop", { width: 1440, height: 900 }],
];

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
    const { writeIssuesRecord } = window.__layoutModules.tracker;
    const { writeAgentsOverviewFixture, overviewRailContext } = window.__layoutModules.fixture;
    await writeAgentsOverviewFixture({ writeCached, stampWorkspace, writeIssuesRecord });
    localStorage.setItem("build.rail.expanded", "1");
    window.__overviewRail = mountAgentRail(document.querySelector("#agent-rail"), overviewRailContext());
  });
  await page.waitForSelector(".rail-overview-toggle", { timeout: 5000 });
  await page.locator(".rail-overview-toggle").click();
  await page.waitForSelector("#rail-panel .rail-overview-list .rail-overview-section", { timeout: 5000 });
  await page.waitForFunction(() => document.querySelectorAll("#rail-panel .rail-overview-section").length >= 5, null, { timeout: 5000 });
}

/** The panel pinned open, as the maintainer's phone had it. */
async function pinned(page) {
  const state = () => page.evaluate(() => ({
    pressed: document.querySelector("#rail-panel .pinbtn")?.getAttribute("aria-pressed"),
    rail: document.querySelector("#agent-rail").className,
  }));
  const before = await state();
  if (before.pressed !== "true") await page.locator("#rail-panel .pinbtn").click();
  // The pin moves the panel with its own animation; the sheet has arrived once
  // it is as wide as the view column and nothing on it is still moving.
  await page.waitForFunction(() => {
    const panel = document.querySelector("#rail-panel");
    const view = document.querySelector("#view").getBoundingClientRect();
    return !document.querySelector("#agent-rail").classList.contains("rail-popover")
      && !panel.getAnimations({ subtree: true }).some((animation) => animation.effect?.getTiming().iterations !== Infinity)
      && (panel.getBoundingClientRect().width >= view.width - 1 || view.width > 760);
  }, null, { timeout: 5000 });
  await page.waitForTimeout(300);
  console.log("pin", JSON.stringify(before), JSON.stringify(await state()));
}

const only = process.env.AO_ONLY || "";
for (const [device, viewport] of VIEWPORTS) {
  for (const theme of ["dark", "light"]) {
    if (only && only !== `${device}-${theme}`) continue;
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, shellHtml(device === "desktop"), { basePath, styles });
      await page.evaluate((wanted) => { document.documentElement.dataset.theme = wanted; }, theme);
      await seed(page, basePath);
      await pinned(page);
      await page.mouse.move(0, 0);
      await settled(page);
      await page.screenshot({ path: join(output, `${label}-${device}-${theme}.png`) });
      const metrics = await page.evaluate(() => {
        const box = (node) => node.getBoundingClientRect().height;
        return {
          rows: [...document.querySelectorAll(".rail-overview-row")].map(box),
          heads: [...document.querySelectorAll(".rail-overview-section-head")].map(box),
          list: document.querySelector(".rail-overview-list")?.scrollHeight,
          adds: [...document.querySelectorAll(".rail-overview-add")].map((node) => [node.getBoundingClientRect().width, node.getBoundingClientRect().height]),
          panel: document.querySelector("#rail-panel")?.getBoundingClientRect().width,
        };
      });
      console.log("%s %s %s %s", label, device, theme, JSON.stringify(metrics));
    }, viewport);
  }
}
console.log(`wrote ${output}`);
