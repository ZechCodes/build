// #257 in a real browser: an agent overview row wears its model's short name
// whole, at every width; the agent's name is what gives way.
import assert from "node:assert/strict";
import { it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// The overview's width in the chat panel: nearly the phone's, or the docked panel's.
const VIEWPORTS = [
  ["narrow phone", { width: 320, height: 640 }, 280],
  ["phone", { width: 390, height: 844 }, 350],
  ["desktop", { width: 1440, height: 900 }, 420],
];

const LONG_NAME = "Divider for markdown horizontal rules in every theme";

for (const [name, viewport, listWidth] of VIEWPORTS) {
  it(`keeps each overview row's model whole beside a long name (${name})`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, `<div class="rail-overview-list" style="width:${listWidth}px"></div>`, { basePath });
      await loadBrowserModules(page, { overview: "src/core/agentOverview.js" }, basePath);
      const read = await page.evaluate((longName) => {
        const { overviewHtml, overviewRows } = window.__layoutModules.overview;
        const entry = (id, model) => ({ agent: { id, name: longName, active_model: model, effort: "xhigh", working: true }, source: "project" });
        const entries = [entry("a1", "claude-opus-5-5"), entry("a2", "gpt-6-astra"), entry("a3", "claude-haiku-4-5-20251001")];
        const list = document.querySelector(".rail-overview-list");
        list.innerHTML = overviewHtml(overviewRows(entries, entries.map(() => null)), { showProjectAgents: true });
        return [...list.querySelectorAll(".rail-overview-model")].map((model) => ({
          text: model.textContent, scrollWidth: model.scrollWidth, clientWidth: model.clientWidth,
          nameClipped: model.previousElementSibling.scrollWidth > model.previousElementSibling.clientWidth,
        }));
      }, LONG_NAME);
      assert.deepEqual(read.map((model) => model.text), ["Opus 5.5", "6 Astra", "Haiku 4.5"]);
      for (const model of read) {
        assert.ok(model.scrollWidth <= model.clientWidth, `the model is not clipped: ${JSON.stringify(model)}`);
        assert.ok(model.nameClipped, `the long name is what gives way: ${JSON.stringify(model)}`);
      }
    }, viewport);
  });
}
