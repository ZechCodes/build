import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

it("persists a debounced draft when Chromium reloads immediately after typing", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<input id="draft" aria-label="Draft">', { basePath });
    await loadBrowserModules(page, { ui: "src/core/localUiState.js", cache: "src/core/localCache.js" }, basePath);
    await page.evaluate(async () => {
      const { uiAddress, watchUiState } = window.__layoutModules.ui;
      const address = uiAddress({ deviceId: "reload-device", entityId: "reload-issue", view: "issue", kind: "draft" });
      await window.__layoutModules.cache.writeCached(address, { body: "older draft" });
      const field = document.querySelector("#draft");
      const record = watchUiState(address, (saved) => { field.value = saved.body; }, { debounceMs: 60_000 });
      field.oninput = () => record.schedule({ body: field.value });
      await record.ready;
    });
    await page.locator("#draft").fill("last keystroke");
    await page.reload();
    await mountLayout(page, '<input id="draft" aria-label="Draft">', { basePath });
    await loadBrowserModules(page, { ui: "src/core/localUiState.js", cache: "src/core/localCache.js" }, basePath);
    const restored = await page.evaluate(async () => {
      const { uiAddress, watchUiState } = window.__layoutModules.ui;
      const address = uiAddress({ deviceId: "reload-device", entityId: "reload-issue", view: "issue", kind: "draft" });
      const field = document.querySelector("#draft");
      const record = watchUiState(address, (saved) => { field.value = saved.body; }, { debounceMs: 60_000 });
      await record.ready;
      return { painted: field.value, cached: (await window.__layoutModules.cache.readCached(address))?.value.body };
    });
    expect(restored).toEqual({ painted: "last keystroke", cached: "last keystroke" });
  });
}, 30_000);
