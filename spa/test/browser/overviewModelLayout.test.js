// #257 in a real browser: an agent overview row wears its model's short name
// whole, at every width; the agent's name is what gives way.
import assert from "node:assert/strict";
import { it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// The overview's width in the chat panel: nearly the phone's, or the docked panel's.
const VIEWPORTS = [
  ["narrow phone", { width: 320, height: 640 }, 280],
  ["phone", { width: 390, height: 844 }, 350],
  ["desktop", { width: 1440, height: 900 }, 420],
];

const RAW_MODEL_ID = "house-research-model-20260915-experimental-preview";
const LONG_NAME = "Divider for markdown horizontal rules in every theme";

for (const [name, viewport, listWidth] of VIEWPORTS) {
  it(`keeps each overview row's model whole and inside its row beside a long name (${name})`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, `<div class="rail-overview-list" style="width:${listWidth}px"></div>`, { basePath });
      await loadBrowserModules(page, { overview: "src/core/agentOverview.js" }, basePath);
      const read = await page.evaluate(({ longName, rawModelId }) => {
        const { overviewHtml, overviewRows } = window.__layoutModules.overview;
        const entry = (id, model) => ({ agent: { id, name: longName, active_model: model, effort: "xhigh", working: true }, source: "project" });
        const entries = [entry("a1", "claude-opus-5-5"), entry("a2", "gpt-6-astra"), entry("a3", "claude-haiku-4-5-20251001"), entry("a4", rawModelId)];
        const list = document.querySelector(".rail-overview-list");
        list.innerHTML = overviewHtml(overviewRows(entries, entries.map(() => null)), { showProjectAgents: true });
        const right = (element) => element.getBoundingClientRect().right;
        return [...list.querySelectorAll(".rail-overview-model")].map((model) => {
          const row = model.closest(".rail-overview-row");
          return {
            text: model.textContent, scrollWidth: model.scrollWidth, clientWidth: model.clientWidth,
            nameClipped: model.previousElementSibling.scrollWidth > model.previousElementSibling.clientWidth,
            modelRight: right(model), whoRight: right(model.parentElement), rowRight: right(row),
            stateLeft: row.querySelector(".rail-overview-state").getBoundingClientRect().left,
            stateRight: right(row.querySelector(".rail-overview-state")),
            height: model.getBoundingClientRect().height,
            lineHeight: parseFloat(getComputedStyle(model).lineHeight),
          };
        });
      }, { longName: LONG_NAME, rawModelId: RAW_MODEL_ID });
      await captureLayout(page, `overview-models-${name}.png`);
      assert.deepEqual(read.map((model) => model.text), ["Opus 5.5", "6 Astra", "Haiku 4.5", RAW_MODEL_ID]);
      for (const model of read.slice(0, 3)) {
        assert.ok(model.nameClipped, `the long name is what gives way: ${JSON.stringify(model)}`);
        assert.ok(model.height < model.lineHeight * 1.5, `a short name keeps one line: ${JSON.stringify(model)}`);
      }
      for (const model of read) {
        assert.ok(model.scrollWidth <= model.clientWidth, `the model is not clipped: ${JSON.stringify(model)}`);
        // Its box ends inside the row, clear of the state column beside it.
        assert.ok(model.modelRight <= model.whoRight + 1 && model.modelRight <= model.stateLeft,
          `the model stays in its column: ${JSON.stringify(model)}`);
        assert.ok(model.stateRight <= model.rowRight + 1, `the state stays inside the row: ${JSON.stringify(model)}`);
      }
    }, viewport);
  });
}
