// Isolated production changes composer evidence (#521), run from spa/.
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { THREE_LINES, mountChangesComposers, settleChangesTheme } from "./changesComposerSeed.mjs";

const phase = process.env.CAPTURE_PHASE || "after";
const directory = process.env.CAPTURE_DIR || "/tmp/changes-composer-521";
await mkdir(directory, { recursive: true });
for (const width of [390, 1280]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountChangesComposers(page, basePath);
    for (const theme of ["light", "dark"]) {
      await settleChangesTheme(page, theme);
      for (const [state, text] of [["empty", ""], ["three-lines", THREE_LINES]]) {
        for (const field of await page.locator(".csinput").all()) await field.fill(text);
        await page.locator(".csinput").last().blur();
        await page.locator("#changes-evidence").screenshot({
          path: join(directory, `${phase}-${width}-${theme}-${state}.png`),
        });
      }
    }
  }, { width, height: 844, hasTouch: width === 390 });
}
