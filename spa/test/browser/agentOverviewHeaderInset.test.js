import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { mountAgentHeader } from "./agentHeaderSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

for (const kind of ["project", "workspace"]) for (const pinned of [undefined, true]) {
  it(`keeps the ${kind} phone Agents title aligned with section headings, ${pinned ? "pinned" : "default unpinned"}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountAgentHeader(page, basePath, { kind, pinned });
      await page.locator('[data-bubble="overview"]').click();
      await page.locator(".rail-overview-section-head h2").first().waitFor();
      expect(await page.locator(".rail-head .rail-who-name").textContent()).toBe("Agents");
      expect(await page.locator(".rail-head .rail-harness-icon").count()).toBe(0);
      expect(await page.locator(".rail-head .pinbtn").getAttribute("aria-pressed")).toBe(String(!!pinned));
      expect(await page.locator("#agent-rail").evaluate((rail) => rail.classList.contains("rail-popover"))).toBe(!pinned);

      const layout = await page.evaluate(() => {
        const head = document.querySelector(".rail-head");
        const title = head.querySelector(".rail-who-name").getBoundingClientRect();
        return {
          padding: parseFloat(getComputedStyle(head).paddingLeft),
          titleInset: title.left - head.getBoundingClientRect().left,
          titleLeft: title.left,
          sectionLefts: [...document.querySelectorAll(".rail-overview-section-head h2")]
            .map((heading) => heading.getBoundingClientRect().left),
        };
      });
      expect(layout.padding).toBe(16);
      expect(layout.titleInset).toBeCloseTo(16, 1);
      expect(layout.sectionLefts.length).toBeGreaterThan(0);
      for (const left of layout.sectionLefts) expect(left).toBeCloseTo(layout.titleLeft, 1);
    }, { width: 390, height: 844, hasTouch: true });
  }, 30_000);
}
