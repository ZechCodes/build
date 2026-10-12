// Isolated Add project evidence for #519; never connects to a real bridge.
// Run from spa/: node test/browser/captureNewProjectProgress.mjs <out-dir>
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { failure, holdProjectCache, mountNewProject, resolveProject } from "./newProjectProgressSeed.mjs";

const directory = process.argv[2] || "/tmp/new-project-progress-519";
await mkdir(directory, { recursive: true });
for (const { label, width, height, hasTouch } of [
  { label: "desktop", width: 1280, height: 800, hasTouch: false },
  { label: "phone", width: 390, height: 844, hasTouch: true },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountNewProject(page, basePath, { local: true });
    await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await page.locator("#nrdo").click();
    await page.waitForSelector("[data-source-state='in_progress']");
    assert.equal(await page.locator("[data-source-state='in_progress']").count(), 2);
    await page.screenshot({ path: join(directory, `${label}-in-progress.png`) });
    await page.evaluate((message) => window.__rejectProject(message), failure);
    await page.waitForSelector("[data-source-state='failed']");
    await page.screenshot({ path: join(directory, `${label}-failed.png`) });
    await page.locator("#nrback").click();
    await page.locator("#nrdo").click();
    await resolveProject(page);
    await page.waitForSelector("[data-source-state='done']");
    await page.screenshot({ path: join(directory, `${label}-success.png`) });
    await page.evaluate(() => window.__releaseProjectCache());
    await page.waitForFunction(() => window.__openedProject);
  }, { width, height, hasTouch, plugins: [deviceShim, holdProjectCache] });
}
