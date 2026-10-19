import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { mountAgentHeader } from "./agentHeaderSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

async function mountChevron(page, basePath, { pinned, badge = false } = {}) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mountAgentHeader(page, basePath, { pinned });
  await page.locator("#inbox-open").evaluate((button, badge) => {
    button.classList.toggle("has-attention", badge);
    button.querySelector(".inbox-open-count").textContent = badge ? "99+" : "";
  }, badge);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  return page.locator(".tb-switch");
}

async function targetPixels(switcher) {
  return switcher.evaluate((button) => {
    const rect = button.getBoundingClientRect();
    const misses = [];
    let sampled = 0;
    for (let y = 0.5; y < 44; y++) for (let x = 0.5; x < 24; x++) {
      sampled++;
      const hit = document.elementFromPoint(rect.left + x, rect.top + y);
      if (!button.contains(hit)) misses.push({ x, y, hit: hit?.id || hit?.className || hit?.tagName });
    }
    return { width: rect.width, height: rect.height, sampled, misses };
  });
}

async function appearance(switcher) {
  return switcher.evaluate((button) => {
    const style = getComputedStyle(button);
    const visual = getComputedStyle(button, "::before");
    const radii = (computed) => ["borderTopLeftRadius", "borderTopRightRadius", "borderBottomRightRadius", "borderBottomLeftRadius"]
      .map((property) => computed[property]);
    const probe = document.createElement("span");
    probe.style.color = "var(--accent)";
    button.append(probe);
    const accent = getComputedStyle(probe).color;
    probe.remove();
    return {
      buttonRadii: radii(style), buttonOutline: style.outlineStyle, colour: style.color,
      visualContent: visual.content, visualRadii: radii(visual), pointerEvents: visual.pointerEvents,
      visualBackground: visual.backgroundColor, outline: visual.outlineStyle,
      outlineWidth: parseFloat(visual.outlineWidth), outlineColour: visual.outlineColor,
      focusVisible: button.matches(":focus-visible"), accent,
    };
  });
}

for (const width of [390, 1280]) for (const pinned of [false, true]) for (const badge of [false, true]) {
  it(`owns every pixel of the chevron's 24×44 target at ${width}px, agent ${pinned ? "pinned" : "unpinned"}, badge ${badge}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      const switcher = await mountChevron(page, basePath, { pinned: !pinned && width <= 760 ? undefined : pinned, badge });
      expect(await page.locator(".rail-head .pinbtn").getAttribute("aria-pressed")).toBe(String(pinned));
      const pixels = await targetPixels(switcher);
      expect(pixels.width).toBe(24);
      expect(pixels.height).toBe(44);
      expect(pixels.sampled).toBe(24 * 44);
      expect(pixels.misses).toEqual([]);
    }, { width, height: 844, hasTouch: true });
  }, 30_000);
}

for (const width of [390, 1280]) {
  it(`opens the switcher from the top-right target corner at ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      const switcher = await mountChevron(page, basePath, { pinned: true, badge: true });
      const target = await switcher.boundingBox();
      await page.touchscreen.tap(target.x + 23, target.y + 1);
      await page.locator(".tb-filter").waitFor({ timeout: 5000 });
      expect(await switcher.getAttribute("aria-expanded")).toBe("true");
      expect(await page.locator("body").evaluate((body) => body.classList.contains("inbox-popover-open"))).toBe(false);
    }, { width, height: 844, hasTouch: true });
  }, 30_000);

  it(`keeps rounded hover and keyboard-focus visuals independent of the rectangular target at ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      const switcher = await mountChevron(page, basePath, { pinned: true });
      await switcher.hover();
      const hover = await appearance(switcher);
      expect.soft(hover.buttonRadii).toEqual(["0px", "0px", "0px", "0px"]);
      expect.soft(hover.visualContent).toBe('""');
      expect.soft(hover.visualRadii).toEqual(["6px", "6px", "6px", "6px"]);
      expect.soft(hover.pointerEvents).toBe("none");
      expect.soft(hover.visualBackground).toBe("rgba(0, 0, 0, 0)");
      expect.soft(hover.colour).toBe(hover.accent);
      expect.soft((await targetPixels(switcher)).misses).toEqual([]);

      await page.mouse.move(0, 0);
      await page.keyboard.press("Tab");
      await switcher.focus();
      const focused = await appearance(switcher);
      expect.soft(focused.focusVisible).toBe(true);
      expect.soft(focused.buttonRadii).toEqual(["0px", "0px", "0px", "0px"]);
      expect.soft(focused.buttonOutline).toBe("none");
      expect.soft(focused.visualRadii).toEqual(["4px", "4px", "4px", "4px"]);
      expect.soft(focused.outline).toBe("solid");
      expect.soft(focused.outlineWidth).toBe(2);
      expect.soft(focused.outlineColour).toBe(focused.accent);
      expect.soft(focused.pointerEvents).toBe("none");
      expect.soft((await targetPixels(switcher)).misses).toEqual([]);
    }, { width, height: 844 });
  }, 30_000);
}
