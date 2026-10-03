import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { openMenuOn, settled } from "./chatMenuSeed.mjs";
import { compactionContinuity, observeCompactionMenu, uninterruptedMenu } from "./compactionMenuContinuity.mjs";

const SLIDER = '.rail-surface-menu [role="slider"]';

async function choose300k(page, gesture) {
  if (gesture === "Enter") {
    for (let stop = 0; stop < 3; stop += 1) await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Enter");
    return;
  }
  const box = await page.locator(SLIDER).boundingBox();
  const y = box.y + box.height / 2;
  if (gesture === "click") {
    await page.mouse.click(box.x + box.width * 0.68, y);
    return;
  }
  await page.mouse.move(box.x + 10, y);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.68, y, { steps: 10 });
  expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([]);
  await page.mouse.up();
}

it.each(["drag", "click", "Enter"])("never closes or replaces the focused menu during a held %s save and its cache reply", async (gesture) => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "dark", bigCounts: false, holdSettings: true });
    await observeCompactionMenu(page);
    await choose300k(page, gesture);
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
    expect(await page.locator(SLIDER).getAttribute("aria-valuetext")).toBe("300k");
    expect(await compactionContinuity(page)).toEqual(uninterruptedMenu);
    await page.evaluate(() => window.__releaseMenuSettings());
    await page.waitForFunction(() => JSON.parse(document.querySelector('[data-group="compact"] .menu-slider').dataset.options)
      .some((option) => option.id === "compact:300000" && option.selected));
    await settled(page);
    expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
    expect(await page.evaluate(() => window.__menuSettingsAsked.map((asked) => asked.max_context_tokens))).toEqual([300000]);
  });
});

it("keeps pointer capture and preview through a cache repaint in the middle of a drag", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "light", bigCounts: false });
    await observeCompactionMenu(page);
    const slider = page.locator(SLIDER);
    await slider.evaluate((element) => element.addEventListener("pointerdown", (event) => {
      window.__dragPointerId = event.pointerId;
    }, { once: true }));
    const box = await slider.boundingBox();
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + 10, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.25, y, { steps: 5 });
    expect(await slider.getAttribute("aria-valuetext")).toBe("150k");
    // Chromium's range thumb owns native capture inside its UA shadow tree;
    // hasPointerCapture on the host input is false after native drag starts.
    // Check continued delivery to this original input outside its bounds, and
    // observe cancellation/capture loss while the cache repaints it.
    await slider.evaluate((element) => {
      window.__dragEvents = [];
      for (const type of ["pointermove", "pointercancel", "lostpointercapture"]) {
        element.addEventListener(type, (event) => window.__dragEvents.push({ type, pointerId: event.pointerId }));
      }
    });
    await page.evaluate(() => window.__repaintMenuAgent({ compact_at_tokens: 220000 }));
    await page.waitForFunction(() => document.querySelector('[data-group="compact"] .menu-slider').dataset.options.includes("Default (220k)"));
    expect(await compactionContinuity(page)).toEqual(uninterruptedMenu);
    expect(await page.evaluate(() => window.__dragEvents)).toEqual([]);
    expect(await slider.getAttribute("aria-valuetext")).toBe("150k");
    await page.mouse.move(box.x + box.width * 0.68, box.y + box.height + 20, { steps: 5 });
    const dragEvents = await page.evaluate(() => window.__dragEvents);
    const pointerId = await page.evaluate(() => window.__dragPointerId);
    expect(dragEvents.length).toBeGreaterThan(0);
    expect(dragEvents.every((event) => event.type === "pointermove")).toBe(true);
    expect(dragEvents.every((event) => event.pointerId === pointerId)).toBe(true);
    expect(await slider.getAttribute("aria-valuetext")).toBe("300k");
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([]);
    await page.mouse.up();
    await page.waitForFunction(() => window.__menuSettingsAnswered.length === 1);
    await page.waitForFunction(() => JSON.parse(document.querySelector('[data-group="compact"] .menu-slider').dataset.options)
      .some((option) => option.id === "compact:300000" && option.selected));
    await settled(page);
    expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
    expect(await page.evaluate(() => window.__menuSettingsAsked.map((asked) => asked.max_context_tokens))).toEqual([300000]);
  });
});
