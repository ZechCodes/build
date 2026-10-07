import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
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
for (const width of [1180, 320]) {
it(`highlights a drop destination without changing row geometry in ${theme} at ${width}px`, async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountFilesExplorer(page, basePath, { theme });
    await loadBrowserModules(page, { support: "src/core/fileUploadSupport.js" }, basePath);
    await page.evaluate(async () => {
      await window.__layoutModules.support.rememberFileUploadSupport("explorer-device", { fs: { uploads: true, createDirectory: true } });
    });
    if (width === 320) {
      await page.locator(".pane-handle").click();
      await page.waitForFunction(() => getComputedStyle(document.querySelector("#ftree")).transform === "none");
    }
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
    await captureLayout(page, `files-upload-drop-${theme}-${width}.png`);
    await page.locator('.frow[data-path="spa"]').evaluate((node) => node.dispatchEvent(new DragEvent("dragleave", { bubbles: true, relatedTarget: document.body })));
    const directory = page.locator('.frow[data-path="spa"]');
    await directory.hover();
    const expanded = await directory.getAttribute("aria-expanded");
    await directory.locator('[data-upload-action="folder"]').click();
    const field = page.getByRole("textbox", { name: "New folder name" });
    await field.fill("assets");
    expect(await directory.getAttribute("aria-expanded")).toBe(expanded);
    expect(await field.inputValue()).toBe("assets");
    expect(await page.locator('.frow[data-path="spa/assets"]').count()).toBe(0);
    expect(await page.locator(".fupload-folder-error").textContent()).toBe("");
    await captureLayout(page, `files-upload-folder-${theme}-${width}.png`);
  }, { width, height: 640 });
}, 60_000);
}
}


it("accepts a native file drop on an unsupported bridge and explains the required update", async () => {
  const directory = await mkdtemp(join(tmpdir(), "build-unsupported-upload-"));
  const filename = join(directory, "hello.txt");
  await writeFile(filename, "hello");
  try {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountFilesExplorer(page, basePath);
      await loadBrowserModules(page, { support: "src/core/fileUploadSupport.js" }, basePath);
      await page.evaluate(async () => {
        await window.__layoutModules.support.rememberFileUploadSupport("explorer-device", { fs: { uploads: false, createDirectory: false } });
        document.addEventListener("dragover", (event) => {
          window.__nativeDropEffect = event.dataTransfer.dropEffect;
          window.__nativeDragTypes = [...event.dataTransfer.types];
        });
      });
      expect(await page.locator("[data-upload-action]").count()).toBe(0);
      const box = await page.locator('.frow[data-path="spa"]').boundingBox();
      const client = await page.context().newCDPSession(page);
      const drag = { x: box.x + 30, y: box.y + box.height / 2, data: { items: [], files: [filename], dragOperationsMask: 1 } };
      await client.send("Input.dispatchDragEvent", { type: "dragEnter", ...drag });
      await client.send("Input.dispatchDragEvent", { type: "dragOver", ...drag });
      expect(await page.evaluate(() => window.__nativeDragTypes)).toContain("Files");
      expect(await page.locator(".fupload-drop").count()).toBe(0);
      expect(await page.evaluate(() => window.__nativeDropEffect)).toBe("copy");
      await client.send("Input.dispatchDragEvent", { type: "drop", ...drag });
      await expect.poll(() => page.locator(".notice-summary").textContent()).toBe("Update the bridge to upload files");
      await captureLayout(page, "files-upload-unsupported-drop.png");
      await client.detach();
    });
  } finally { await rm(directory, { recursive: true, force: true }); }
}, 60_000);
