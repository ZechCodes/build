import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

const toolbarControls = [".tb-switch", ".tb-project", ".tb-tasks", ".tb-leaf", ".tb-sel"];
const headerControls = ['[data-inbox-view="inbox"]', '[data-inbox-view="projects"]', "#inbox-new-project", "#inbox-collapse"];

for (const width of [390, 1280]) for (const theme of ["light", "dark"]) for (const state of ["collapsed", "open", "popover"]) {
  it(`every control in the visible navigation bar uses ink-only hover ${state} ${width}px ${theme}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationBreadcrumbs(page, basePath, { theme, kind: "projectTask", inboxHeader: true });
      await page.evaluate(async (state) => {
        await window.__layoutModules.inbox.setInboxCollapsed(state !== "open", { animate: false, persist: false, reveal: false });
        if (state === "popover") document.querySelector("#inbox-open").click();
      }, state);
      await page.waitForTimeout(300);
      expect(await page.evaluate(() => document.body.classList.contains("inbox-collapsed"))).toBe(state !== "open");
      expect(await page.evaluate(() => document.body.classList.contains("inbox-popover-open"))).toBe(state === "popover");
      const controls = await page.evaluate(() => {
        const toolbar = document.querySelector("#toolbar").getBoundingClientRect();
        // Include the global controls to the left of #toolbar. The picker and
        // body controls lie outside this visible row, regardless of ancestry.
        return [...document.querySelectorAll('button, a[href], [role="button"], [role="tab"]')]
          .filter((element) => {
            const rect = element.getBoundingClientRect();
            return rect.width > 0 && rect.height > 0 && rect.top >= toolbar.top && rect.bottom <= toolbar.bottom
              && rect.left >= 0 && rect.right <= toolbar.right && getComputedStyle(element).visibility === "visible"
              && element.contains(document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2));
          }).map((element) => element.id ? `#${element.id}` : element.dataset.inboxView
            ? `[data-inbox-view="${element.dataset.inboxView}"]` : `.${element.classList[0]}`);
      });
      const docked = width > 900 && state === "open";
      const expected = docked ? [...headerControls, ...toolbarControls]
        : width <= 900 && state !== "collapsed" ? ["#inbox-open"] : ["#inbox-open", ...toolbarControls];
      expect(controls).toEqual(expected);
      for (const selector of controls) {
        const control = page.locator(selector);
        await page.mouse.move(width - 1, 843);
        const before = await control.boundingBox();
        const restingBackground = await control.evaluate((element) => getComputedStyle(element).backgroundColor);
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
        expect.soft(hover.colour, selector).toBe(accent);
        // Selection may keep its indication; hovering must not add any fill.
        if (selector === headerControls[0]) {
          expect(await control.getAttribute("aria-pressed")).toBe("true");
          expect(restingBackground, selector).not.toBe("rgba(0, 0, 0, 0)");
          expect(hover.background, selector).toBe(restingBackground);
        } else expect.soft(hover.background, selector).toBe("rgba(0, 0, 0, 0)");
        expect.soft(hover.shadow, selector).toBe("none");
        expect.soft(hover.icons.every((colour) => colour === accent), selector).toBe(true);
        expect(await control.boundingBox(), selector).toEqual(before);
        await page.mouse.move(width - 1, 843);
        await page.keyboard.press("Tab");
        await control.focus();
        const focus = await control.evaluate((element) => {
          const style = getComputedStyle(element, element.matches(".tb-switch") ? "::before" : null);
          return { visible: element.matches(":focus-visible"), outline: style.outlineStyle,
            width: parseFloat(style.outlineWidth), colour: style.outlineColor };
        });
        expect(focus.visible, selector).toBe(true);
        expect(focus.outline, selector).toBe("solid");
        expect(focus.width, selector).toBeGreaterThanOrEqual(2);
        expect(focus.colour, selector).toBe(accent);
      }
      if (docked) {
        await page.locator(headerControls[1]).click();
        await page.mouse.move(width - 1, 843);
        const selected = page.locator(headerControls[1]);
        await expect.poll(() => selected.getAttribute("aria-pressed")).toBe("true");
        const background = await selected.evaluate((element) => getComputedStyle(element).backgroundColor);
        expect(background).not.toBe("rgba(0, 0, 0, 0)");
        await selected.hover();
        expect(await selected.evaluate((element) => getComputedStyle(element).backgroundColor)).toBe(background);
        expect.soft(await selected.evaluate((element) => getComputedStyle(element).boxShadow)).toBe("none");
      }
    }, { width, height: 844 });
  }, 30_000);
}

for (const width of [390, 1280]) for (const theme of ["light", "dark"]) {
  it(`inbox popover keeps its outside-bar hover treatment ${width}px ${theme}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationBreadcrumbs(page, basePath, { theme, kind: "projectTask", inboxHeader: true });
      await page.evaluate(() => window.__layoutModules.inbox.setInboxCollapsed(true, { animate: false, persist: false, reveal: true }));
      await page.waitForTimeout(300);
      const bar = await page.locator("#toolbar").boundingBox();
      for (const selector of headerControls) {
        const control = page.locator(selector);
        if (!await control.isVisible()) continue; // Phone overlays have no pin.
        const bounds = await control.boundingBox();
        expect(bounds.y).toBeGreaterThan(bar.y + bar.height);
        await control.hover();
        await page.waitForTimeout(200);
        const style = await control.evaluate((element) => {
          const computed = getComputedStyle(element);
          return { shadow: computed.boxShadow, background: computed.backgroundColor };
        });
        expect(style.shadow, selector).toContain("inset");
        expect(style.background, selector).not.toBe("rgba(0, 0, 0, 0)");
      }
    }, { width, height: 844 });
  }, 30_000);
}
