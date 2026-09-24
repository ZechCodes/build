// Capture the Files tab's explorer for #151, in both themes: the desktop with
// two levels expanded, three tabs open and one holding unsaved edits; and a
// phone with a file open and the tree's drawer dropped over it (and shut,
// showing the tabs).
// Run from spa/: node test/browser/captureFilesExplorer.mjs [output directory]
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountFilesExplorer, openFile, stageDesktop } from "./filesExplorerSeed.mjs";

const output = resolve(process.argv[2] || "/tmp/files-explorer");
await mkdir(output, { recursive: true });

for (const theme of ["dark", "light"]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountFilesExplorer(page, basePath, { theme });
    await stageDesktop(page);
    await page.screenshot({ path: `${output}/desktop-${theme}.png` });

    const phone = await page.context().browser().newContext({
      viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, hasTouch: true, isMobile: true,
    });
    const tap = await phone.newPage();
    await tap.goto(page.url());
    await mountFilesExplorer(tap, basePath, { theme });
    await tap.locator("[data-pane-handle]").tap();
    await openFile(tap, "AGENTS.md", { touch: true });
    await tap.locator("[data-pane-handle]").tap();
    await openFile(tap, "README.md", { touch: true });
    await tap.waitForTimeout(400);
    await tap.screenshot({ path: `${output}/phone-closed-${theme}.png` });
    await tap.locator("[data-pane-handle]").tap();
    await tap.waitForTimeout(400);
    await tap.screenshot({ path: `${output}/phone-${theme}.png` });
    await phone.close();
  }, { width: 1320, height: 850 });
}
console.log(output);
