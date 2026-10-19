import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { mountAgentHeader } from "./agentHeaderSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

for (const kind of ["project", "workspace"]) for (const pinned of [undefined, true]) {
  for (const directory of [true, false]) for (const inset of [0, 20]) {
    it(`aligns ${kind} phone glyphs, ${pinned ? "pinned" : "default unpinned"}, directory ${directory}, safe area ${inset}px`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await page.emulateMedia({ reducedMotion: "reduce" });
        await mountAgentHeader(page, basePath, { kind, pinned });
        expect(await page.locator(".rail-head .pinbtn").getAttribute("aria-pressed")).toBe(String(!!pinned));
        expect(await page.locator("#agent-rail").evaluate((rail) => rail.classList.contains("rail-popover"))).toBe(!pinned);
        if (!directory) await page.locator("#dir-rail").evaluate((rail) => rail.replaceChildren());
        const cdp = await page.context().newCDPSession(page);
        await cdp.send("Emulation.setSafeAreaInsetsOverride", { insets: { left: inset } });
        await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
        const positions = await page.evaluate(() => ({
          inbox: document.querySelector("#inbox-open svg").getBoundingClientRect().left,
          harness: document.querySelector(".rail-head .rail-harness-icon svg").getBoundingClientRect().left,
          overflow: document.documentElement.scrollWidth > innerWidth,
        }));
        expect(positions.inbox).toBe(21.5 + (directory ? 0 : inset));
        expect(positions.harness).toBeCloseTo(positions.inbox, 1);
        expect(positions.overflow).toBe(false);
        expect(await page.locator(".rail-head .agent-model-badge").count()).toBe(0);
      }, { width: 390, height: 844 });
    }, 30_000);
  }
}

for (const width of [761, 1280]) {
  it(`retains main's 16px desktop inset for conversation and Agents overview at ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountAgentHeader(page, basePath);
      const inset = () => page.locator(".rail-head").evaluate((head) => parseFloat(getComputedStyle(head).paddingLeft));
      expect(await inset()).toBe(16);
      await page.locator('[data-bubble="overview"]').click();
      await page.locator(".rail-overview-list").waitFor();
      expect(await inset()).toBe(16);
    }, { width, height: 844 });
  }, 30_000);
}

for (const width of [320, 390, 1280]) for (const directory of [true, false]) for (const inset of [0, 20]) {
  it(`matches toolbar gaps with adjoining hit targets at ${width}px, directory ${directory}, safe area ${inset}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountAgentHeader(page, basePath);
      if (!directory) await page.locator("#dir-rail").evaluate((rail) => rail.replaceChildren());
      const cdp = await page.context().newCDPSession(page);
      await cdp.send("Emulation.setSafeAreaInsetsOverride", { insets: { left: inset } });
      await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      const layout = await page.evaluate(() => {
        const inbox = document.querySelector("#inbox-open");
        const switcher = document.querySelector(".tb-switch");
        const rect = (element) => element.getBoundingClientRect();
        const toggle = rect(inbox);
        const target = rect(switcher);
        const extension = getComputedStyle(inbox, "::before");
        const inboxTarget = {
          left: toggle.left + parseFloat(extension.left), right: toggle.right - parseFloat(extension.right),
          top: toggle.top + parseFloat(extension.top), bottom: toggle.bottom - parseFloat(extension.bottom),
        };
        const inboxGlyph = rect(inbox.querySelector("svg"));
        const glyph = rect(switcher.querySelector("svg"));
        const project = rect(document.querySelector(".tb-project .tb-name"));
        const workspace = rect(document.querySelector('[data-select="workspace"] .tb-name'));
        const midpoint = (inboxGlyph.right + glyph.left) / 2;
        const missedHits = [];
        // Scan both halves of the visible gap along the full 44px height.
        // This catches overlapping targets and dead strips, not just boxes.
        for (let y = 0.5; y < 44; y++) {
          for (let x = inboxGlyph.right + 1; x < glyph.left; x++) {
            const owner = x < midpoint ? inbox : switcher;
            if (!owner.contains(document.elementFromPoint(x, target.top + y))) missedHits.push({ x, y });
          }
        }
        return {
          glyphGap: glyph.left - inboxGlyph.right, otherGap: workspace.left - project.right,
          targetWidth: target.width, targetHeight: target.height,
          inboxWidth: inboxTarget.right - inboxTarget.left, inboxHeight: inboxTarget.bottom - inboxTarget.top,
          seam: target.left - inboxTarget.right, midpointOffset: target.left - midpoint,
          missedHits: missedHits.slice(0, 5), midpoint, centerY: target.top + target.height / 2,
          toolbarHeight: rect(document.querySelector("#toolbar")).height,
          overflow: document.documentElement.scrollWidth > innerWidth,
        };
      });
      expect(layout.otherGap).toBe(12);
      expect(Math.abs(layout.glyphGap - layout.otherGap)).toBeLessThanOrEqual(1);
      expect(layout.glyphGap).toBeLessThan(31.5);
      expect(layout.targetWidth).toBe(24);
      expect(layout.targetHeight).toBe(44);
      expect(layout.inboxWidth).toBe(44);
      expect(layout.inboxHeight).toBe(44);
      expect(layout.seam).toBe(0);
      expect(layout.midpointOffset).toBe(0);
      expect(layout.missedHits).toEqual([]);
      expect(layout.toolbarHeight).toBe(44);
      expect(layout.overflow).toBe(false);
      await page.touchscreen.tap(layout.midpoint + 1, layout.centerY);
      await page.locator(".tb-filter").waitFor();
      await page.keyboard.press("Escape");
      await page.touchscreen.tap(layout.midpoint - 1, layout.centerY);
      expect(await page.locator("body").evaluate((body) => body.classList.contains("inbox-popover-open"))).toBe(true);
    }, { width, height: 844, hasTouch: true });
  }, 30_000);
}
