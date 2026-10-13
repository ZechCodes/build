import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

for (const width of [390, 1280]) for (const theme of ["light", "dark"]) {
  it(`every control in the visible navigation bar uses ink-only hover ${width}px ${theme}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationBreadcrumbs(page, basePath, { theme, kind: "projectTask" });
      await page.evaluate(() => document.body.classList.add("inbox-collapsed"));
      await page.waitForTimeout(300);
      const controls = await page.evaluate(() => {
        const toolbar = document.querySelector("#toolbar").getBoundingClientRect();
        // Include the global controls to the left of #toolbar. The picker and
        // body controls lie outside this visible row, regardless of ancestry.
        return [...document.querySelectorAll('button, a[href], [role="button"], [role="tab"]')]
          .filter((element) => {
            const rect = element.getBoundingClientRect();
            return rect.width > 0 && rect.height > 0 && rect.top >= toolbar.top && rect.bottom <= toolbar.bottom
              && rect.left >= 0 && rect.right <= toolbar.right && getComputedStyle(element).visibility === "visible";
          }).map((element) => element.id ? `#${element.id}` : `.${element.classList[0]}`);
      });
      expect(controls).toEqual(["#inbox-open", ".tb-switch", ".tb-project", ".tb-tasks", ".tb-leaf", ".tb-sel"]);
      for (const selector of controls) {
        const control = page.locator(selector);
        await page.mouse.move(width - 1, 843);
        const before = await control.boundingBox();
        await control.hover();
        await page.waitForTimeout(200);
        const hover = await control.evaluate((element) => {
          const style = getComputedStyle(element);
          return { colour: style.color, accent: style.getPropertyValue("--accent").trim(),
            background: style.backgroundColor, shadow: style.boxShadow,
            icons: [...element.querySelectorAll("svg, .tb-caret")].map((icon) => getComputedStyle(icon).color) };
        });
        const accent = await page.evaluate((token) => {
          const probe = document.createElement("span"); probe.style.color = token; document.body.append(probe);
          const colour = getComputedStyle(probe).color; probe.remove(); return colour;
        }, hover.accent);
        expect(hover.colour, selector).toBe(accent);
        expect(hover.background, selector).toBe("rgba(0, 0, 0, 0)");
        expect(hover.shadow, selector).toBe("none");
        expect(hover.icons.every((colour) => colour === accent), selector).toBe(true);
        expect(await control.boundingBox(), selector).toEqual(before);
        await page.mouse.move(width - 1, 843);
        await page.keyboard.press("Tab");
        await control.focus();
        const focus = await control.evaluate((element) => {
          const style = getComputedStyle(element);
          return { visible: element.matches(":focus-visible"), outline: style.outlineStyle,
            width: parseFloat(style.outlineWidth), colour: style.outlineColor };
        });
        expect(focus.visible, selector).toBe(true);
        expect(focus.outline, selector).toBe("solid");
        expect(focus.width, selector).toBeGreaterThanOrEqual(2);
        expect(focus.colour, selector).toBe(accent);
      }
    }, { width, height: 844 });
  }, 30_000);
}
