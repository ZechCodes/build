import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { mountAgentHeader } from "./agentHeaderSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

for (const width of [1280, 390, 320]) {
  it(`aligns header and inbox glyph insets and keeps compact, separate toolbar targets at ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountAgentHeader(page, basePath);
      const layout = await page.evaluate(() => {
        const rect = (selector) => document.querySelector(selector).getBoundingClientRect();
        const inbox = rect("#inbox-open svg");
        const chevron = rect(".tb-switch svg");
        const harness = rect(".rail-head .rail-harness-icon svg");
        const target = rect(".tb-switch");
        return {
          inboxInset: inbox.left - rect("#view").left,
          harnessInset: harness.left - rect(".rail-panel").left,
          aligned: harness.left - inbox.left,
          glyphGap: chevron.left - inbox.right,
          targetGap: target.left - rect("#inbox-open").right,
          projectGap: rect(".tb-project").left - target.right,
          targetWidth: target.width, targetHeight: target.height,
          toolbarHeight: rect("#toolbar").height,
          overflow: document.documentElement.scrollWidth > innerWidth,
        };
      });
      // Desktop panels are side by side; on phones the header is directly
      // below the toolbar, so the same local inset must also align on screen.
      expect(layout.harnessInset).toBeCloseTo(layout.inboxInset, 1);
      if (width <= 760) expect(layout.aligned).toBeCloseTo(0, 1);
      expect(layout.glyphGap).toBeGreaterThanOrEqual(12);
      expect(layout.glyphGap).toBeLessThanOrEqual(20);
      expect(layout.targetGap).toBeGreaterThanOrEqual(0);
      expect(layout.projectGap).toBe(4);
      expect(layout.targetWidth).toBeGreaterThanOrEqual(28);
      expect(layout.targetHeight).toBe(44);
      expect(layout.toolbarHeight).toBe(44);
      expect(layout.overflow).toBe(false);
      expect(await page.locator(".rail-head .agent-model-badge").count()).toBe(0);
      await page.locator(".tb-switch").click();
      await page.locator(".tb-filter").waitFor();
      await page.keyboard.press("Escape");
      await page.locator("#inbox-open").click();
      expect(await page.locator("body").evaluate((body) => body.classList.contains("inbox-popover-open"))).toBe(true);
    }, { width, height: 844 });
  }, 30_000);
}

for (const directory of [true, false]) it(`aligns phone header with safe-area inbox, directory rail ${directory}`, async () => {
  await session.withPage(async ({ page, basePath }) => {
    const cdp = await page.context().newCDPSession(page);
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountAgentHeader(page, basePath);
    if (!directory) await page.locator("#dir-rail").evaluate((rail) => rail.replaceChildren());
    await cdp.send("Emulation.setSafeAreaInsetsOverride", { insets: { left: 20 } });
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const positions = await page.evaluate(() => ({
      inbox: document.querySelector("#inbox-open svg").getBoundingClientRect().left,
      harness: document.querySelector(".rail-head .rail-harness-icon svg").getBoundingClientRect().left,
    }));
    expect(positions.inbox).toBe(directory ? 21.5 : 41.5);
    expect(positions.harness).toBeCloseTo(positions.inbox, 1);
  }, { width: 390, height: 844 });
}, 30_000);
