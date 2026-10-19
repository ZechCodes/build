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

const edgeSamples = [
  ["bottom-right", 23.5, 43.5], ["bottom-left", 0.5, 43.5],
  ["top-left", 0.5, 0.5], ["top-right", 23.5, 0.5],
  ["bottom", 12, 43.5], ["top", 12, 0.5],
  ["left", 0.5, 22], ["right", 23.5, 22],
];

async function recordInput(page) {
  await page.evaluate(() => {
    window.__chevronInput = [];
    for (const type of ["pointerdown", "pointerup", "click"]) {
      document.addEventListener(type, (event) => window.__chevronInput.push({
        type, chevron: Boolean(event.target.closest(".tb-switch")),
        railHead: Boolean(event.target.closest(".rail-head")),
        target: event.target.id || event.target.className || event.target.tagName,
      }), { capture: true });
    }
  });
}

async function dispatchTap(cdp, input, x, y) {
  if (input === "touch") {
    await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y, radiusX: 1, radiusY: 1 }] });
    await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  } else {
    await cdp.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
    await cdp.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
  }
}

for (const width of [390, 1280]) for (const pinned of [false, true]) for (const badge of [false, true])
  for (const deviceScaleFactor of [1, 2]) for (const input of ["touch", "mouse"]) {
  it(`opens from every corner and edge with ${input} at ${width}px, pinned ${pinned}, badge ${badge}, DPR ${deviceScaleFactor}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      const switcher = await mountChevron(page, basePath, { pinned, badge });
      expect(await page.locator(".rail-head .pinbtn").getAttribute("aria-pressed")).toBe(String(pinned));
      const target = await switcher.boundingBox();
      expect([target.width, target.height]).toEqual([24, 44]);
      await recordInput(page);
      const cdp = await page.context().newCDPSession(page);
      for (const [edge, x, y] of edgeSamples) {
        await page.evaluate(() => { window.__chevronInput = []; });
        await dispatchTap(cdp, input, target.x + x, target.y + y);
        await page.waitForFunction(() => window.__chevronInput.some((event) => event.type === "click"));
        const events = await page.evaluate(() => window.__chevronInput);
        expect.soft(events.map((event) => event.type), edge).toEqual(["pointerdown", "pointerup", "click"]);
        expect.soft(events.filter((event) => !event.chevron || event.railHead), edge).toEqual([]);
        expect.soft(await switcher.getAttribute("aria-expanded"), edge).toBe("true");
        expect.soft(await page.locator(".tb-filter").isVisible(), edge).toBe(true);
        await page.keyboard.press("Escape");
        expect(await switcher.getAttribute("aria-expanded")).toBe("false");
      }
      expect((await targetPixels(switcher)).misses).toEqual([]);
      await cdp.detach();
    }, { width, height: 844, hasTouch: true, deviceScaleFactor });
  }, 30_000);
}

for (const width of [390, 1280]) {
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
