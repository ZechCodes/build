import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { openMenuOn, settled } from "./chatMenuSeed.mjs";

const SLIDER = '.rail-surface-menu [role="slider"]';
const CARET = ".rail-surface-menu .caret";

async function reopen(page) {
  await page.waitForFunction(() => {
    const last = window.__menuSettingsAsked.at(-1);
    if (!last) return true;
    const limit = last.max_context_tokens;
    const id = limit === null ? "compact:default" : limit === 0 ? "compact:off" : `compact:${limit}`;
    return document.querySelector('.rail-surface-menu [role="slider"]')?.dataset.action === id;
  });
  await settled(page);
  await page.locator(CARET).click();
  await page.waitForFunction(() => !document.querySelector(".rail-surface-menu .splitmenu").hidden);
  await settled(page);
}

async function word(page) {
  return page.locator(SLIDER).getAttribute("aria-valuetext");
}

it("snaps pointer drags to stops on release, saves once, and shows one value at 320px", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "narrow", { theme: "dark", bigCounts: false });
    const slider = page.locator(SLIDER);
    expect(await slider.count()).toBe(1);
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
    await reopen(page);
    expect(await word(page)).toBe("300k");
    expect(await page.locator('[data-group="compact"] .mt').allTextContents()).toEqual(["300k"]);
    const description = await page.locator('[data-group="compact"] .md').evaluate((element) => {
      const range = document.createRange();
      range.selectNodeContents(element);
      return { lines: range.getClientRects().length, fits: element.scrollWidth <= element.clientWidth };
    });
    expect(description).toEqual({ lines: 1, fits: true });
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
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([]);
    await reopen(page);
    expect(await word(page)).toBe("Default (200k)");
    await slider.focus();
    for (let stop = 0; stop < 4; stop += 1) await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([
      { entity_id: "menu-run", agent_id: "menu-agent", max_context_tokens: 0 },
    ]);
    await reopen(page);
    expect(await word(page)).toBe("Off");
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
    await reopen(page);
    expect(await word(page)).toBe("200k");
    await session.detach();
  }, { width: 390, height: 844 });
});

it("scrolls the slider's heading and selected description into view with the focused thumb", async () => {
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
      const description = group.querySelector(".md").getBoundingClientRect();
      return { heading: heading.top >= top - 1, description: description.bottom <= bottom + 1 };
    });
    expect(visible).toEqual({ heading: true, description: true });
  }, { width: 320, height: 480 });
});
