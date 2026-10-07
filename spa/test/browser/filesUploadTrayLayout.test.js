import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";
import { mountFilesExplorer } from "./filesExplorerSeed.mjs";

async function seedTray(page, basePath) {
  await loadBrowserModules(page, { tray: "src/core/fileUploadTray.js" }, basePath);
  await page.evaluate(() => {
    window.__files.dispose();
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
        await captureLayout(page, `files-upload-active-${theme}-${width}.png`);
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

for (const theme of ["light", "dark"]) {
it(`highlights a drop destination without changing row geometry in ${theme}`, async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountFilesExplorer(page, basePath, { theme });
    await loadBrowserModules(page, { support: "src/core/fileUploadSupport.js" }, basePath);
    await page.evaluate(async () => {
      await window.__layoutModules.support.rememberFileUploadSupport("explorer-device", { fs: { uploads: true, createDirectory: true } });
    });
    await page.locator('.frow[data-path="spa"] [data-upload-action="upload"]').waitFor();
    const geometry = await page.evaluate(() => {
      const row = document.querySelector('.frow[data-path="spa"]');
      const measure = () => { const box = row.getBoundingClientRect(); return { x: box.x, y: box.y, width: box.width, height: box.height }; };
      const allBefore = [...document.querySelectorAll(".frow")].map((node) => { const box = node.getBoundingClientRect(); return { x: box.x, y: box.y, width: box.width, height: box.height }; });
      const before = measure();
      const dataTransfer = new DataTransfer();
      dataTransfer.items.add(new File(["hello"], "hello.txt"));
      row.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer }));
      const allAfter = [...document.querySelectorAll(".frow")].map((node) => { const box = node.getBoundingClientRect(); return { x: box.x, y: box.y, width: box.width, height: box.height }; });
      const actual = getComputedStyle(row).backgroundColor;
      const probe = document.createElement("span");
      probe.style.backgroundColor = "var(--row-hover)";
      row.append(probe);
      const expected = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return { before, allBefore, allAfter, actual, expected, after: measure(), highlighted: row.classList.contains("fupload-drop"), hint: row.dataset.uploadHint };
    });
    expect(geometry.highlighted).toBe(true);
    expect(geometry.hint).toBeTruthy();
    expect(geometry.after).toEqual(geometry.before);
    expect(geometry.allAfter).toEqual(geometry.allBefore);
    await expect.poll(() => page.locator('.frow[data-path="spa"]').evaluate((node) => getComputedStyle(node).backgroundColor)).toBe(geometry.expected);
  });
}, 60_000);
}
