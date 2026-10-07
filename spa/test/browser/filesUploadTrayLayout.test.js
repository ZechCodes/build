import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";
import { mountFilesExplorer } from "./filesExplorerSeed.mjs";

async function seedTray(page, basePath) {
  await loadBrowserModules(page, { tray: "src/core/fileUploadTray.js" }, basePath);
  await page.evaluate(() => {
    const active = [1, 2, 3].map((id) => ({ id: String(id), name: `design-${id}.png`, size: 1000, received: 420, parent: "spa/src", destination: "spa/src", status: "uploading" }));
    const recent = [{ id: "failed", name: "existing.png", size: 800, received: 0, destination: "spa/src", status: "failed", error: "A file with this name already exists", errorCode: "already_exists", canRetry: true, finishedAt: Date.now() }];
    window.__trayCalls = [];
    const uploads = { snapshot: () => ({ active, recent }), subscribe: () => () => {}, prune() {}, cancel: (id) => window.__trayCalls.push(["cancel", id]), retry: (id, options) => window.__trayCalls.push(["retry", id, options]) };
    window.__disposeTray = window.__layoutModules.tray.mountFileUploadTray(document.querySelector("#fpreview"), { uploads });
  });
}

for (const theme of ["light", "dark"]) {
  for (const width of [1180, 320]) {
    it(`confines the upload tray to the viewer at ${width}px in ${theme}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await mountFilesExplorer(page, basePath, { theme });
        await captureLayout(page, `files-upload-before-${theme}-${width}.png`);
        await seedTray(page, basePath);
        await expect.poll(() => page.locator(".fupload-summary").textContent()).toBe("Uploading 3 files · 42%");
        await page.locator(".fupload-summary").focus();
        await page.keyboard.press("Enter");
        await page.locator(".fupload-recent-toggle").click();
        const box = await page.evaluate(() => {
          const viewer = document.querySelector("#fpreview").getBoundingClientRect();
          const tray = document.querySelector(".fupload-tray").getBoundingClientRect();
          return { left: tray.left - viewer.left, top: tray.top - viewer.top, right: viewer.right - tray.right, bottom: viewer.bottom - tray.bottom, width: tray.width };
        });
        expect(box.left).toBeGreaterThanOrEqual(0);
        expect(box.top).toBeGreaterThanOrEqual(0);
        expect(box.right).toBeCloseTo(12, 0);
        expect(box.bottom).toBeCloseTo(12, 0);
        expect(box.width).toBeLessThanOrEqual(320);
        expect(await page.locator(".fupload-item").count()).toBe(4);
        await page.getByRole("button", { name: "Replace existing.png" }).click();
        expect(await page.evaluate(() => window.__trayCalls)).toEqual([["retry", "failed", { replace: true }]]);
        await captureLayout(page, `files-upload-after-${theme}-${width}.png`);
      }, { width, height: 640 });
    }, 60_000);
  }
}
