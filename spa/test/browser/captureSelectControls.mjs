// Optional review artifacts: five representative production surfaces, their
// controls and open native pickers, at phone/desktop widths in both themes.
// BUILD_LAYOUT_SCREENSHOTS=/tmp/select-captures nice -n 10 node test/browser/captureSelectControls.mjs
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountSelectSurface, openSelectDisclosures } from "./selectSurfaces.mjs";

const directory = process.env.BUILD_LAYOUT_SCREENSHOTS;
if (!directory) throw new Error("Set BUILD_LAYOUT_SCREENSHOTS to the review artifact directory");
await mkdir(directory, { recursive: true });
const surfaces = [
  { name: "clear conversation", label: "conversation", selector: "[data-clear-detail]" },
  { name: "local settings", label: "settings", selector: "#creationdev" },
  { name: "workspace settings", label: "sheet", selector: "#wsdiradd" },
  { name: "task controls", label: "diff-sort", selector: ".diffsort-select" },
  { name: "task review", label: "task-review", selector: "[data-review-snapshot]" },
];

for (const width of [320, 1280]) {
  for (const theme of ["light", "dark"]) {
    await withLayoutPage(async ({ page, basePath }) => {
      const origin = new URL(page.url()).origin;
      for (const surface of surfaces) {
        await page.goto(`${origin}${basePath}src/styles.css`);
        await mountSelectSurface(page, basePath, surface.name, theme);
        await openSelectDisclosures(page);
        const select = page.locator(surface.selector).first();
        await select.scrollIntoViewIfNeeded();
        await page.screenshot({ path: join(directory, `${surface.label}-${width}-${theme}-closed.png`) });
        await select.focus();
        await page.keyboard.press("Space");
        await page.waitForFunction(() => !!document.querySelector("select:open"));
        await page.screenshot({ path: join(directory, `${surface.label}-${width}-${theme}-open.png`) });
        await page.keyboard.press("Escape");
      }
    }, { width, height: 900 });
  }
}
