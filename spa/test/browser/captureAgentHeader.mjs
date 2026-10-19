import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { mountAgentHeader } from "./agentHeaderSeed.mjs";

const directory = process.argv[2] || "/tmp/build-agent-header-662";
await mkdir(directory, { recursive: true });
const session = await openLayoutSession({ plugins: [navigationAppShim] });
try {
  for (const { width, pinned, name } of [
    { width: 1280, name: "desktop" },
    { width: 390, pinned: true, name: "phone-pinned" },
    { width: 390, name: "phone-unpinned" },
  ]) {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountAgentHeader(page, basePath, { pinned });
      for (const view of ["header", "overview"]) {
        if (view === "overview") {
          await page.locator('[data-bubble="overview"]').click();
          await page.locator(".rail-overview-list").waitFor();
        }
        for (const theme of ["light", "dark"]) {
          await page.evaluate((theme) => {
            document.documentElement.dataset.theme = theme;
            document.querySelector("#inbox-open").classList.add("has-attention");
            document.querySelector(".inbox-open-count").textContent = "2";
            for (const animation of document.getAnimations()) {
              if (animation instanceof CSSTransition) animation.finish();
            }
          }, theme);
          await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
          await page.screenshot({ path: join(directory, `agent-${view}-${name}-${theme}.png`) });
          if (view === "header") {
            const switcher = page.locator(".tb-switch");
            await switcher.hover();
            await page.screenshot({ path: join(directory, `agent-chevron-hover-${name}-${theme}.png`) });
            await page.mouse.move(0, 0);
            await page.keyboard.press("Tab");
            await switcher.focus();
            await page.screenshot({ path: join(directory, `agent-chevron-focus-${name}-${theme}.png`) });
            await switcher.evaluate((button) => button.blur());
          }
        }
      }
    }, { width, height: 844 });
  }
} finally {
  await session.close();
}
