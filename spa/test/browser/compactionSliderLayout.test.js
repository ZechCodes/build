import { expect, it } from "vitest";
import { captureLayout, withLayoutPage } from "./layoutHarness.mjs";
import { openMenuOn, settled } from "./chatMenuSeed.mjs";
import { compactionContinuity, observeCompactionMenu, uninterruptedMenu } from "./compactionMenuContinuity.mjs";

const SLIDER = '.rail-surface-menu [role="slider"]';
const CARET = ".rail-surface-menu .caret";

async function savedMenuOpen(page) {
  await page.waitForFunction(() => {
    const last = window.__menuSettingsAsked.at(-1);
    if (!last) return true;
    const limit = last.max_context_tokens;
    const id = limit === null ? "compact:default" : limit === 0 ? "compact:off" : `compact:${limit}`;
    return document.querySelector('.rail-surface-menu [role="slider"]')?.dataset.action === id;
  });
  await settled(page);
  expect(await page.locator(CARET).getAttribute("aria-expanded")).toBe("true");
  expect(await page.locator(".rail-surface-menu .splitmenu").evaluate((menu) => menu.hidden)).toBe(false);
  expect(await page.locator(SLIDER).evaluate((slider) => document.activeElement === slider)).toBe(true);
}

async function reopen(page) {
  await page.locator(CARET).click();
  await page.waitForFunction(() => !document.querySelector(".rail-surface-menu .splitmenu").hidden);
  await settled(page);
}

async function word(page) {
  return page.locator(SLIDER).getAttribute("aria-valuetext");
}

it.each(["pointer", "Enter"])("keeps the chosen stop visible while a %s save is held, then handles a late answer without resending", async (gesture) => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "dark", bigCounts: false, resetCapable: true, holdSettings: true });
    const slider = page.locator(SLIDER);
    await observeCompactionMenu(page);
    if (gesture === "pointer") {
      const box = await slider.boundingBox();
      await page.mouse.click(box.x + box.width * 0.68, box.y + box.height / 2);
    } else {
      await slider.focus();
      for (let stop = 0; stop < 3; stop += 1) await page.keyboard.press("ArrowRight");
      await page.keyboard.press("Enter");
    }
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
    expect(await word(page)).toBe("300k");
    expect(await page.locator('[data-group="compact"] .mt').textContent()).toBe("300k");
    expect(await slider.evaluate((element) => document.activeElement === element)).toBe(true);
    expect(await page.locator(CARET).getAttribute("aria-expanded")).toBe("true");
    await captureLayout(page, `compaction-300k-pending-${gesture.toLowerCase()}.png`);
    if (gesture === "pointer") {
      expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
      await page.keyboard.press("Escape");
      await page.evaluate(() => window.__releaseMenuSettings());
      await page.waitForFunction(() => document.querySelector('.rail-surface-menu [role="slider"]')?.dataset.action === "compact:300000");
    } else {
      await page.evaluate(() => window.__refuseMenuSettings());
      await page.waitForFunction(() => document.querySelector('.rail-surface-menu [role="slider"]')?.getAttribute("aria-valuetext") === "Default (200k)");
      expect(await slider.evaluate((element) => document.activeElement === element)).toBe(true);
      expect(await page.locator('[data-group="compact"] .mt').textContent()).toBe("Default (200k)");
      expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
      await page.keyboard.press("Escape");
    }
    await settled(page);
    expect(await page.locator(CARET).getAttribute("aria-expanded")).toBe("false");
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toHaveLength(1);
  });
});

it("snaps pointer drags to stops on release, saves once, and shows one value at 320px", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "narrow", { theme: "dark", bigCounts: false });
    const slider = page.locator(SLIDER);
    expect(await slider.count()).toBe(1);
    await observeCompactionMenu(page);
    const box = await slider.boundingBox();
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + 10, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.68, y, { steps: 10 });
    expect(await word(page)).toBe("300k");
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([]);
    await page.mouse.up();
    await page.waitForFunction(() => window.__menuSettingsAsked?.length === 1);
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([
      { entity_id: "menu-run", agent_id: "menu-agent", max_context_tokens: 300000 },
    ]);
    await savedMenuOpen(page);
    expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
    expect(await word(page)).toBe("300k");
    expect(await page.locator('[data-group="compact"] .mt').allTextContents()).toEqual(["300k"]);
    expect(await page.locator('[data-group="compact"] .md').count()).toBe(0);
    expect(await page.locator('[data-group="compact"] .menu-slider-stops span').count()).toBe(5);
    expect(await slider.getAttribute("aria-label")).toBe("Compact at");
    await captureLayout(page, "compaction-after-drag-320.png");
    const menuBox = await page.locator(".rail-surface-menu .splitmenu").boundingBox();
    expect(menuBox.x).toBeGreaterThanOrEqual(0);
    expect(menuBox.x + menuBox.width).toBeLessThanOrEqual(320);
  }, { width: 320, height: 640 });
});

it("previews every stop in one open menu, writes once on Enter, and cancels on Escape", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "light", bigCounts: false });
    const slider = page.locator(SLIDER);
    await slider.focus();
    await page.keyboard.press("ArrowLeft");
    expect(await word(page)).toBe("Default (200k)");
    for (const expected of ["150k", "200k", "300k", "Off", "Off"]) {
      await page.keyboard.press("ArrowRight");
      expect(await word(page)).toBe(expected);
      expect(await slider.evaluate((element) => document.activeElement === element)).toBe(true);
      expect(await page.locator(CARET).getAttribute("aria-expanded")).toBe("true");
      expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([]);
    }
    await page.keyboard.press("Escape");
    await settled(page);
    expect(await page.locator(CARET).getAttribute("aria-expanded")).toBe("false");
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([]);
    await reopen(page);
    expect(await word(page)).toBe("Default (200k)");
    await observeCompactionMenu(page);
    for (let stop = 0; stop < 4; stop += 1) await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([
      { entity_id: "menu-run", agent_id: "menu-agent", max_context_tokens: 0 },
    ]);
    await savedMenuOpen(page);
    expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
    expect(await word(page)).toBe("Off");
    await captureLayout(page, "compaction-after-enter.png");
    await slider.focus();
    await page.keyboard.press("ArrowLeft");
    expect(await word(page)).toBe("300k");
    await page.keyboard.press("Escape");
    await settled(page);
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toHaveLength(1);
  });
});

it("commits a keyboard preview on Tab and preserves the destination focus through the cache reply", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "light", bigCounts: false });
    await page.evaluate(() => {
      const button = document.createElement("button");
      button.id = "after-menu";
      button.textContent = "After menu";
      document.querySelector(".rail-surface-menu").insertAdjacentElement("afterend", button);
    });
    await page.locator(SLIDER).focus();
    await page.keyboard.press("ArrowRight");
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([]);
    await page.keyboard.press("Tab");
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
    await page.waitForFunction(() => document.querySelector('.rail-surface-menu [role="slider"]')?.dataset.action === "compact:150000");
    expect(await page.evaluate(() => document.activeElement?.id)).toBe("after-menu");
    expect(await page.locator(CARET).getAttribute("aria-expanded")).toBe("false");
  });
});

it("keeps the menu navigation destination after a keyboard blur saves its preview", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "light", bigCounts: false });
    await page.locator(SLIDER).focus();
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("ArrowUp");
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
    await page.waitForFunction(() => document.querySelector('.rail-surface-menu [role="slider"]')?.dataset.action === "compact:150000");
    await settled(page);
    expect(await page.evaluate(() => document.activeElement?.dataset.action)).toBe("detail:agent");
    expect(await page.locator(CARET).getAttribute("aria-expanded")).toBe("true");
  });
});

it("saves a clicked stop with slider focus, then saves a preview while walking down to Clear conversation", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "dark", bigCounts: false, resetCapable: true });
    const slider = page.locator(SLIDER);
    await observeCompactionMenu(page);
    const box = await slider.boundingBox();
    await page.mouse.click(box.x + box.width * 0.39, box.y + box.height / 2);
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
    await savedMenuOpen(page);
    expect(await compactionContinuity(page, { stop: true })).toEqual(uninterruptedMenu);
    expect(await word(page)).toBe("200k");
    await captureLayout(page, "compaction-after-click-with-clear.png");
    await page.keyboard.press("ArrowRight");
    expect(await word(page)).toBe("300k");
    await page.keyboard.press("ArrowDown");
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 2);
    await page.waitForFunction(() => document.querySelector('.rail-surface-menu [role="slider"]')?.dataset.action === "compact:300000");
    await settled(page);
    expect(await page.evaluate(() => document.activeElement?.dataset.action)).toBe("conversation:clear");
    expect(await page.locator(CARET).getAttribute("aria-expanded")).toBe("true");
    await captureLayout(page, "compaction-after-change-with-clear.png");
    await page.keyboard.press("ArrowUp");
    expect(await slider.evaluate((element) => document.activeElement === element)).toBe(true);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await page.getByRole("dialog").waitFor();
    expect(await page.getByRole("dialog").textContent()).toContain("Clear this conversation?");
    expect(await page.evaluate(() => window.__menuSettingsAsked.map((asked) => asked.max_context_tokens))).toEqual([200000, 300000]);
  });
});

it("commits an outside click once and stays closed after the cache reply", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "light", bigCounts: false });
    await page.locator(SLIDER).focus();
    await page.keyboard.press("ArrowRight");
    await page.locator("#toolbar").click();
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
    await page.waitForFunction(() => document.querySelector('.rail-surface-menu [role="slider"]')?.dataset.action === "compact:150000");
    await settled(page);
    expect(await page.locator(CARET).getAttribute("aria-expanded")).toBe("false");
    expect(await page.locator(".rail-surface-menu .splitmenu").evaluate((menu) => menu.hidden)).toBe(true);
    expect(await page.evaluate(() => window.__menuSettingsAsked.map((asked) => asked.max_context_tokens))).toEqual([150000]);
  });
});

it("snaps a real touch drag to the nearest stop without scrolling the menu", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "mobile", { theme: "light", bigCounts: false });
    const slider = page.locator(SLIDER);
    expect(await slider.count()).toBe(1);
    const box = await slider.boundingBox();
    const session = await page.context().newCDPSession(page);
    await session.send("Emulation.setTouchEmulationEnabled", { enabled: true });
    const y = box.y + box.height / 2;
    const touch = async (type, x) => session.send("Input.dispatchTouchEvent", {
      type, touchPoints: type === "touchEnd" ? [] : [{ x, y }],
    });
    await touch("touchStart", box.x + 10);
    await touch("touchMove", box.x + box.width * 0.39);
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([]);
    await touch("touchEnd");
    await page.waitForFunction(() => window.__menuSettingsAsked?.length === 1);
    expect(await page.evaluate(() => window.__menuSettingsAsked[0].max_context_tokens)).toBe(200000);
    await savedMenuOpen(page);
    expect(await word(page)).toBe("200k");
    await session.detach();
  }, { width: 390, height: 844 });
});

it("scrolls the slider's heading and selected value into view with the focused thumb", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "narrow", { theme: "dark", bigCounts: false });
    await page.keyboard.press("Escape");
    await settled(page);
    await page.locator(CARET).focus();
    await page.keyboard.press("ArrowUp");
    await settled(page);
    const visible = await page.evaluate(() => {
      const menu = document.querySelector(".rail-surface-menu .splitmenu");
      const group = menu.querySelector('[data-group="compact"]');
      const top = menu.getBoundingClientRect().top + menu.clientTop;
      const bottom = top + menu.clientHeight;
      const heading = group.querySelector(".menu-group-title").getBoundingClientRect();
      const value = group.querySelector(".mt").getBoundingClientRect();
      return { heading: heading.top >= top - 1, value: value.bottom <= bottom + 1 };
    });
    expect(visible).toEqual({ heading: true, value: true });
  }, { width: 320, height: 480 });
});
