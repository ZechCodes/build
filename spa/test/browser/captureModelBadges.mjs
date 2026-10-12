// #539: isolated production task headers and agent rail, both sizes and themes.
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountModelBadges } from "./modelBadgeSeed.mjs";

const directory = process.env.CAPTURE_DIR || "/tmp/agent-model-badge-539";
await mkdir(directory, { recursive: true });
for (const { label, width, height, hasTouch } of [
  { label: "phone", width: 390, height: 844, hasTouch: true },
  { label: "desktop", width: 1280, height: 800, hasTouch: false },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountModelBadges(page, basePath);
    for (const theme of ["light", "dark"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      await page.evaluate(() => Promise.all(document.getAnimations().map((animation) => {
        if (animation instanceof CSSTransition) animation.finish();
        return animation instanceof CSSTransition ? animation.finished : Promise.resolve();
      })));
      await page.mouse.move(0, 0);
      await page.locator("#rail-panel").screenshot({ path: join(directory, `${label}-${theme}-rail.png`) });
      await page.locator(".rail-overview-toggle").click();
      await page.locator("#badge-task").screenshot({ path: join(directory, `${label}-${theme}-comments.png`) });
      await page.locator(".rail-overview-toggle").click();
    }
  }, { width, height, hasTouch });
}
