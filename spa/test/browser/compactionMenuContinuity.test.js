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

it("retains the focused menu and every surviving row when cached tasks appear and disappear", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "dark", bigCounts: false });
    await observeCompactionMenu(page);
    expect(await page.locator('.rail-surface-menu [data-action="tasks"]').count()).toBe(0);
    await page.evaluate(() => window.__setMenuTasks([{
      id: "menu-task", number: 366, title: "Keep the menu open", state: "open", status: "in_progress",
      assignee: { kind: "agent", agent_id: "menu-agent" },
    }]));
    await page.locator('.rail-surface-menu [data-action="tasks"]').waitFor({ state: "attached" });
    await settled(page);
    expect(await compactionContinuity(page)).toEqual(uninterruptedMenu);
    await page.evaluate(() => window.__setMenuTasks([]));
    await page.locator('.rail-surface-menu [data-action="tasks"]').waitFor({ state: "detached" });
    await settled(page);
    expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([]);
  });
});

async function menuBounds(page) {
  return page.evaluate(() => {
    const menu = document.querySelector(".rail-surface-menu .splitmenu");
    const box = menu.getBoundingClientRect();
    const panel = document.querySelector("#rail-panel").getBoundingClientRect();
    return { top: box.top, bottom: box.bottom, bottomBound: Math.min(window.innerHeight, panel.bottom),
      position: menu.style.position, opensAbove: menu.style.top === "auto" };
  });
}

it.each(["bottom", "top"])("keeps a lifted menu inside its %s gutter when a cached Tasks row increases its height", async (edge) => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "dark", bigCounts: false });
    const menuHeight = await page.locator(".rail-surface-menu .splitmenu").evaluate((menu) => menu.getBoundingClientRect().height);
    await page.keyboard.press("Escape");
    await settled(page);
    await page.evaluate(({ edge, menuHeight }) => {
      const panel = document.querySelector("#rail-panel");
      const panelTop = panel.getBoundingClientRect().top;
      const caret = document.querySelector(".rail-surface-menu .caret").getBoundingClientRect();
      if (edge === "bottom") {
        // The unchanged menu fits below its button with only four pixels
        // beyond the required eight-pixel rail gutter.
        panel.style.bottom = "auto";
        panel.style.height = `${caret.bottom + 6 + menuHeight + 12 - panelTop}px`;
      } else {
        // Moving the real header leaves four pixels of slack above an
        // upward-opening menu; its fixed-position containing block remains
        // the production glass header with its backdrop-filter.
        document.querySelector(".rail-head").style.top = `${menuHeight + 18 - (caret.top - panelTop)}px`;
      }
    }, { edge, menuHeight });
    await page.locator(".rail-surface-menu .caret").click();
    await settled(page);
    await observeCompactionMenu(page);
    const before = await menuBounds(page);
    expect(before.position).toBe("fixed");
    expect(before.opensAbove).toBe(edge === "top");
    expect(before.top).toBeGreaterThanOrEqual(7.5);
    expect(before.bottom).toBeLessThanOrEqual(before.bottomBound - 7.5);
    await page.evaluate(() => window.__setMenuTasks([{
      id: "menu-task", number: 366, title: "Keep the menu open", state: "open", status: "in_progress",
      assignee: { kind: "agent", agent_id: "menu-agent" },
    }]));
    await page.locator('.rail-surface-menu [data-action="tasks"]').waitFor({ state: "attached" });
    await settled(page);
    expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
    const after = await menuBounds(page);
    expect(after.top).toBeGreaterThanOrEqual(7.5);
    expect(after.bottom).toBeLessThanOrEqual(after.bottomBound - 7.5);
  });
});

it("keeps the same focused menu inside its bound when Tasks arrive during its opening reveal", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "dark", bigCounts: false });
    const menuHeight = await page.locator(".rail-surface-menu .splitmenu").evaluate((menu) => menu.getBoundingClientRect().height);
    await page.keyboard.press("Escape");
    await settled(page);
    await page.evaluate((menuHeight) => {
      const panel = document.querySelector("#rail-panel");
      const caret = document.querySelector(".rail-surface-menu .caret").getBoundingClientRect();
      panel.style.bottom = "auto";
      panel.style.height = `${caret.bottom + 6 + menuHeight + 12 - panel.getBoundingClientRect().top}px`;
    }, menuHeight);
    const opening = await page.evaluate(async () => {
      document.querySelector(".rail-surface-menu .caret").click();
      await new Promise(requestAnimationFrame);
      const menu = document.querySelector(".rail-surface-menu .splitmenu");
      const animation = menu.getAnimations().find((animation) => animation.effect.target === menu);
      if (!animation) throw new Error("The production opening reveal must be running");
      animation.pause();
      animation.currentTime = animation.effect.getTiming().duration / 3;
      window.__openingAnimation = animation;
      menu.querySelector('[role="slider"]').focus({ preventScroll: true });
      return { height: menu.getBoundingClientRect().height, paused: animation.playState === "paused" };
    });
    expect(opening.paused).toBe(true);
    expect(opening.height).toBeGreaterThan(0);
    expect(opening.height).toBeLessThan(menuHeight);
    // The original reveal began before this observer. Any further animation,
    // close, replacement, or focus loss from the cache update is a violation.
    await observeCompactionMenu(page);
    await page.evaluate(() => window.__setMenuTasks([{
      id: "menu-task", number: 366, title: "Keep the menu open", state: "open", status: "in_progress",
      assignee: { kind: "agent", agent_id: "menu-agent" },
    }]));
    await page.locator('.rail-surface-menu [data-action="tasks"]').waitFor({ state: "attached" });
    expect(await compactionContinuity(page)).toEqual(uninterruptedMenu);
    await page.evaluate(() => window.__openingAnimation.play());
    await settled(page);
    expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
    const after = await menuBounds(page);
    expect(after.top).toBeGreaterThanOrEqual(7.5);
    expect(after.bottom).toBeLessThanOrEqual(after.bottomBound - 7.5);
  });
});
