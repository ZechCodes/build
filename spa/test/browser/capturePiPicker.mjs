// Chromium proof for #394: the production new-agent picker and the shared
// creation panel, with Pi installed and no models or CLI version.
// Run from spa/: nice -n 10 node test/browser/capturePiPicker.mjs [output dir]
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const output = process.argv[2] || "/tmp/pi-picker-captures";
await mkdir(output, { recursive: true });
const catalog = {
  default_provider: "claude_adk",
  agent_modes: { claude: "headless", codex: "headless" },
  providers: [
    { id: "claude_adk", label: "Claude Code", installed: true, models: [], efforts: ["low", "high"] },
    { id: "codex_app_server", label: "Codex", installed: true, models: [], efforts: ["low", "medium", "high"] },
    { id: "pi", label: "Pi", installed: true, models: [], efforts: ["off", "minimal", "low", "medium", "high", "xhigh", "max"], cli_name: "pi", cli_version: null },
  ],
};
const markup = `<div id="shell"><div id="view"><header id="toolbar">Build · Workspace</header>
  <div id="view-body"><main id="root"><h1>Build workspace</h1></main>
  <aside id="agent-rail" aria-label="Agents"></aside></div></div></div>`;

for (const theme of ["light", "dark"]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, markup, { basePath, styles: "#toolbar{padding:12px 20px} #root{padding:24px}" });
    await loadBrowserModules(page, {
      rail: "src/core/agentRail.js", cache: "src/core/localCache.js",
      choice: "src/core/agentChoice.js", records: "src/core/settingsRecords.js",
      contexts: "src/core/deviceContexts.js",
    }, basePath);
    await page.evaluate(async ({ catalog, theme }) => {
      document.documentElement.dataset.theme = theme;
      const m = window.__layoutModules;
      const deviceId = "pi-picker-device";
      m.contexts.knownDeviceContext(deviceId);
      await m.cache.writeCached(m.records.deviceModelsAddress(deviceId), catalog);
      await m.cache.writeCached({ deviceId, entityId: "pi-picker-run", kind: "row" }, {
        entity_id: "pi-picker-run", project_id: "pi-picker-project", agents: [],
      });
      localStorage.setItem("build.rail.expanded", "1");
      window.__piRail = m.rail.mountAgentRail(document.querySelector("#agent-rail"), {
        kind: "workspace", deviceId, projectId: "pi-picker-project", workspaceId: "pi-picker-workspace",
        entityId: "pi-picker-run", addingAgent: true,
        call: async (method) => method === "models.list" ? catalog : { items: [] },
      });
    }, { catalog, theme });
    await page.locator('.rail-harness-choice[data-provider="pi"]').click();
    await page.waitForFunction(() => document.querySelector('.rail-harness-choice[data-provider="pi"]')?.getAttribute("aria-pressed") === "true");
    assert.deepEqual(await page.locator(".rail-harness-choice").allTextContents().then((labels) => labels.map((label) => label.trim())), ["Claude Code", "Codex", "Pi"]);
    await page.waitForFunction(() => document.getAnimations().every((animation) => animation.effect?.getTiming().iterations === Infinity));
    await page.screenshot({ path: join(output, `pi-agent-picker-${theme}.png`), animations: "disabled" });
    await page.locator(".composer-reasoning .caret").click();
    await page.screenshot({ path: join(output, `pi-efforts-${theme}.png`), animations: "disabled" });
    await page.keyboard.press("Escape");

    // The compose/assignment creation panel uses the same catalog and params.
    await page.evaluate(({ catalog }) => {
      const choice = window.__layoutModules.choice;
      document.querySelector("#root").innerHTML = `<div class="panel">${choice.agentChoicePanelHtml(
        catalog, { provider: "pi", model: "", effort: "high" }, { open: true },
      )}</div>`;
    }, { catalog });
    assert.equal(await page.locator("#agent-choice-model").isDisabled(), true);
    assert.deepEqual(await page.evaluate(({ catalog }) => {
      const choice = window.__layoutModules.choice;
      return choice.agentChoiceParams(catalog, choice.readAgentChoice(document.querySelector("#root")));
    }, { catalog }), { provider: "pi", effort: "high" });
    await page.screenshot({ path: join(output, `pi-creation-panel-${theme}.png`), animations: "disabled" });
    await page.evaluate(() => window.__piRail.dispose());
  }, { width: 1180, height: 840 });
}
console.log(`wrote ${output}`);
