import { readFileSync } from "node:fs";
import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";

const html = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
const body = html.match(/<body>([\s\S]*)<\/body>/)[1];
let session;
beforeAll(async () => {
  session = await openLayoutSession({ plugins: [navigationAppShim, {
    name: "dev-channel-fixture",
    config: () => ({ define: { "import.meta.env.VITE_BUILD_CHANNEL": JSON.stringify("dev") } }),
  }] });
}, 30_000);
afterAll(async () => session?.close());

for (const width of [375, 1280]) {
  for (const gated of [true, false]) {
    it(`keeps Dev visible without covering controls at ${width}px, gated=${gated}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await page.emulateMedia({ reducedMotion: "reduce" });
        if (gated) {
          await mountLayout(page, body, { basePath });
          await page.evaluate(() => {
            document.body.classList.add("gated");
            document.querySelector("#root").innerHTML = '<div class="shell-stub"><h2>Welcome to Build</h2></div>';
          });
        } else await mountNavigationBreadcrumbs(page, basePath, { inboxHeader: true });
        await loadBrowserModules(page, { channel: "src/core/buildChannel.js" }, basePath);
        await page.evaluate(() => window.__layoutModules.channel.initBuildChannel());
        const badge = page.locator("#build-channel");
        expect(await badge.isVisible()).toBe(true);
        expect(await page.title()).toBe("Build Dev — review is the product");
        const rect = await badge.boundingBox();
        expect(rect.x).toBeGreaterThanOrEqual(0);
        expect(rect.x + rect.width).toBeLessThanOrEqual(width);
        expect(rect.height).toBeGreaterThan(10);
        for (const selector of ["#inbox-open", ".inbox-head button", ".toolbar button"]) {
          for (const control of await page.locator(selector).all()) {
            if (!await control.isVisible()) continue;
            const box = await control.boundingBox();
            const overlap = Math.min(rect.x + rect.width, box.x + box.width) > Math.max(rect.x, box.x)
              && Math.min(rect.y + rect.height, box.y + box.height) > Math.max(rect.y, box.y);
            expect(overlap, selector).toBe(false);
          }
        }
        await captureLayout(page, `build-channel-${width}-${gated ? "welcome" : "shell"}.png`);
      }, { width, height: 820 });
    }, 30_000);
  }
}
