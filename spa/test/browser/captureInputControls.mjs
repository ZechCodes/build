// Optional review artifacts; each frame mounts production renderers and CSS.
// BUILD_LAYOUT_SCREENSHOTS=/tmp/input-captures nice -n 10 node test/browser/captureInputControls.mjs
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountInputSurface } from "./inputSurfaces.mjs";
import { settled } from "./chatMenuSeed.mjs";

const directory = process.env.BUILD_LAYOUT_SCREENSHOTS;
if (!directory) throw new Error("Set BUILD_LAYOUT_SCREENSHOTS to the review artifact directory");
await mkdir(directory, { recursive: true });
const surfaces = [
  { name: "task review", selector: "[data-review-base]" },
  { name: "task controls", selector: "#capture-branch" },
  { name: "workspace folder", selector: "#wslabel" },
  { name: "new project remote", selector: "[data-source-value]" },
  { name: "device settings", selector: "input[name=model]" },
  { name: "task composer", selector: ".task-compose-title" },
];
for (const width of [320, 1280]) {
  for (const theme of ["light", "dark"]) {
    await withLayoutPage(async ({ page, basePath }) => {
      const origin = new URL(page.url()).origin;
      for (const surface of surfaces) {
        await page.goto(`${origin}${basePath}src/styles.css`);
        await mountInputSurface(page, basePath, surface.name, theme);
        const control = page.locator(surface.selector).first();
        await control.scrollIntoViewIfNeeded();
        await page.mouse.move(0, 0);
        await page.evaluate(() => document.activeElement?.blur());
        await settled(page);
        await page.screenshot({ path: join(directory, `${surface.name.replaceAll(" ", "-")}-${width}-${theme}.png`) });
        await page.keyboard.press("Tab");
        await control.focus();
        await settled(page);
        await page.screenshot({ path: join(directory, `${surface.name.replaceAll(" ", "-")}-${width}-${theme}-focus.png`) });
        await page.evaluate(() => window.__inputDispose?.());
        await page.waitForLoadState("networkidle");
      }
    }, { width, height: 900 });
  }
}
