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

for (const width of [320, 390]) {
  it(`tightens main's phone glyph gap while preserving separate 44px targets at ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountAgentHeader(page, basePath);
      const layout = await page.evaluate(() => {
        const inbox = document.querySelector("#inbox-open");
        const switcher = document.querySelector(".tb-switch");
        const rect = (element) => element.getBoundingClientRect();
        const toggle = rect(inbox);
        const target = rect(switcher);
        const inboxGlyph = rect(inbox.querySelector("svg"));
        const glyph = rect(switcher.querySelector("svg"));
        // The floating inbox keeps its visual 28px box; its hit extension
        // uses the empty margin to the left, stopping at the chevron target.
        const inboxTarget = { left: toggle.right - 44, top: toggle.top - 8 };
        const missedHits = (element, box) => {
          const misses = [];
          for (let y = 1; y < 44; y++) for (let x = 1; x < 44; x++) {
            // Rounded button corners need not own the corner pixels.
            if ((x < 6 || x > 38) && (y < 6 || y > 38)) continue;
            const hit = document.elementFromPoint(box.left + x, box.top + y);
            if (!element.contains(hit)) misses.push({ x, y, hit: hit?.id || hit?.className });
          }
          return misses.slice(0, 5);
        };
        return {
          glyphGap: glyph.left - inboxGlyph.right,
          targetWidth: target.width, targetHeight: target.height,
          targetGap: target.left - toggle.right,
          inboxMisses: missedHits(inbox, inboxTarget),
          switcherMisses: missedHits(switcher, target),
          toolbarHeight: rect(document.querySelector("#toolbar")).height,
          overflow: document.documentElement.scrollWidth > innerWidth,
        };
      });
      // Main used a 48px clearance plus 26px chevron leading padding.
      expect(layout.glyphGap).toBeLessThan(45.5);
      expect(layout.glyphGap).toBe(31.5);
      expect(layout.targetWidth).toBe(44);
      expect(layout.targetHeight).toBe(44);
      expect(layout.targetGap).toBeGreaterThanOrEqual(0);
      expect(layout.inboxMisses).toEqual([]);
      expect(layout.switcherMisses).toEqual([]);
      expect(layout.toolbarHeight).toBe(44);
      expect(layout.overflow).toBe(false);
      await page.locator(".tb-switch").tap();
      await page.locator(".tb-filter").waitFor();
      await page.keyboard.press("Escape");
      const toggle = await page.locator("#inbox-open").boundingBox();
      await page.touchscreen.tap(toggle.x - 12, toggle.y + 14);
      expect(await page.locator("body").evaluate((body) => body.classList.contains("inbox-popover-open"))).toBe(true);
    }, { width, height: 844, hasTouch: true });
  }, 30_000);
}
