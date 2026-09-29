import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

it("persists a debounced draft when Chromium reloads immediately after typing", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<input id="draft" aria-label="Draft">', { basePath });
    await loadBrowserModules(page, { ui: "src/core/localUiState.js", cache: "src/core/localUiStore.js" }, basePath);
    await page.evaluate(async () => {
      const { uiAddress, watchUiState } = window.__layoutModules.ui;
      const address = uiAddress({ deviceId: "reload-device", entityId: "reload-task", view: "task", kind: "draft" });
      await window.__layoutModules.cache.writeUiRecord(address, { body: "older draft" });
      const field = document.querySelector("#draft");
      const record = watchUiState(address, (saved) => { field.value = saved.body; }, { debounceMs: 60_000 });
      field.oninput = () => record.schedule({ body: field.value });
      await record.ready;
    });
    await page.locator("#draft").fill("last keystroke");
    await page.reload();
    await mountLayout(page, '<input id="draft" aria-label="Draft">', { basePath });
    await loadBrowserModules(page, { ui: "src/core/localUiState.js", cache: "src/core/localUiStore.js" }, basePath);
    const restored = await page.evaluate(async () => {
      const { uiAddress, watchUiState } = window.__layoutModules.ui;
      const address = uiAddress({ deviceId: "reload-device", entityId: "reload-task", view: "task", kind: "draft" });
      const field = document.querySelector("#draft");
      const record = watchUiState(address, (saved) => { field.value = saved.body; }, { debounceMs: 60_000 });
      await record.ready;
      return { painted: field.value, cached: (await window.__layoutModules.cache.readUiRecord(address))?.value.body };
    });
    expect(restored).toEqual({ painted: "last keystroke", cached: "last keystroke" });
  });
}, 30_000);

it("keeps a competing committed draft after local readback and a real reload", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<input id="draft" aria-label="Draft">', { basePath });
    await loadBrowserModules(page, { ui: "src/core/localUiState.js", cache: "src/core/localUiStore.js" }, basePath);
    await page.evaluate(async () => {
      const { uiAddress, watchUiState } = window.__layoutModules.ui;
      const cache = window.__layoutModules.cache;
      const address = uiAddress({ deviceId: "reload-device", entityId: "competing-draft", view: "task", kind: "draft" });
      let finishCompeting;
      const competing = new Promise((resolve) => { finishCompeting = resolve; });
      let raced = false;
      cache.subscribeUiRecords(address, () => {
        if (raced) return;
        raced = true;
        void cache.writeUiRecord(address, { body: "newer other writer" }).then(finishCompeting);
      });
      const field = document.querySelector("#draft");
      window.__paints = [];
      const record = watchUiState(address, (saved) => {
        field.value = saved.body;
        window.__paints.push(saved.body);
      }, { debounceMs: 60_000 });
      await record.ready;
      await record.write({ body: "old local writer" });
      await competing;
    });
    await page.waitForFunction(() => document.querySelector("#draft").value === "newer other writer");
    const beforeReload = await page.evaluate(async () => {
      const address = window.__layoutModules.ui.uiAddress({ deviceId: "reload-device", entityId: "competing-draft", view: "task", kind: "draft" });
      window.dispatchEvent(new Event("pagehide"));
      return {
        painted: document.querySelector("#draft").value,
        paints: window.__paints,
        cached: (await window.__layoutModules.cache.readUiRecord(address))?.value.body,
        journal: sessionStorage.getItem(`build.ui.pending:${JSON.stringify([address.deviceId, address.entityId, address.kind, address.sub])}`),
      };
    });
    expect(beforeReload).toMatchObject({ painted: "newer other writer", cached: "newer other writer", journal: null });
    expect(beforeReload.paints).not.toContain("old local writer");

    await page.reload();
    await mountLayout(page, '<input id="draft" aria-label="Draft">', { basePath });
    await loadBrowserModules(page, { ui: "src/core/localUiState.js", cache: "src/core/localUiStore.js" }, basePath);
    const restored = await page.evaluate(async () => {
      const { uiAddress, watchUiState } = window.__layoutModules.ui;
      const address = uiAddress({ deviceId: "reload-device", entityId: "competing-draft", view: "task", kind: "draft" });
      const field = document.querySelector("#draft");
      const record = watchUiState(address, (saved) => { field.value = saved.body; }, { debounceMs: 60_000 });
      await record.ready;
      return { painted: field.value, cached: (await window.__layoutModules.cache.readUiRecord(address))?.value.body };
    });
    expect(restored).toEqual({ painted: "newer other writer", cached: "newer other writer" });
  });
}, 30_000);
