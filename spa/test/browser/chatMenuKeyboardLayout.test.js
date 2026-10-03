// The conversation head's ⋮ menu driven from the keyboard, in a real Chromium
// at the narrowest phone width (#124 review, P2): the menu is taller than the
// rail allows there, so it scrolls inside itself, and the row the keys land on
// has to be scrolled into sight — Enter chooses whatever holds focus, seen or
// not. Layout is the whole question, so jsdom cannot ask it.
import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountChatMenu, settled } from "./chatMenuSeed.mjs";

const CARET = ".rail-surface-menu .caret";
const MENU = ".rail-surface-menu .splitmenu";

/** The row holding focus and where it stands: inside the menu's padding box,
 *  which is what scrolls, to within a pixel of rounding. */
const focusedRow = (page) => page.evaluate((selector) => {
  const menu = document.querySelector(selector);
  const row = document.activeElement?.closest('.mi, [role="slider"]');
  if (!row) return { action: null };
  const top = menu.getBoundingClientRect().top + menu.clientTop;
  const box = row.getBoundingClientRect();
  return {
    action: row.dataset.action,
    inside: box.top >= top - 1 && box.bottom <= top + menu.clientHeight + 1,
    scrollTop: Math.round(menu.scrollTop),
    scrolls: menu.scrollHeight > menu.clientHeight,
  };
}, MENU);

const pressed = async (page, key) => {
  await page.keyboard.press(key);
  await settled(page);
  return focusedRow(page);
};

it("keeps the row the keyboard lands on inside the scrolling menu at 320×640", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChatMenu(page, basePath, "narrow", { theme: "light", bigCounts: false });
    const actions = await page.evaluate((selector) =>
      [...document.querySelectorAll(`${selector} .mi, ${selector} [role="slider"]`)].map((row) => row.dataset.action), MENU);
    const first = actions[0];
    const last = actions.at(-1);
    expect(last).toBe("compact:default");
    await page.locator(CARET).focus();

    // Opening on the last row: the menu has more rows than room, and the
    // row is brought in by scrolling the menu itself.
    const opened = await pressed(page, "ArrowUp");
    expect(opened).toMatchObject({ action: last, inside: true });
    await page.keyboard.press("Escape");
    await settled(page);
    expect(await pressed(page, "ArrowDown")).toMatchObject({ action: first, inside: true, scrollTop: 0 });

    // Home and End jump to either end; the arrows wrap around both.
    expect(await pressed(page, "Home")).toMatchObject({ action: first, inside: true, scrollTop: 0 });
    expect(await pressed(page, "End")).toMatchObject({ action: last, inside: true });
    expect(await pressed(page, "ArrowDown")).toMatchObject({ action: first, inside: true, scrollTop: 0 });
    expect(await pressed(page, "ArrowUp")).toMatchObject({ action: last, inside: true });

    // Every row from the top, one arrow at a time, comes into sight in turn.
    await pressed(page, "Home");
    for (const action of actions.slice(1)) {
      expect(await pressed(page, "ArrowDown")).toMatchObject({ action, inside: true });
    }

    // Home after the reader scrolled the menu by hand with the first row focused.
    await pressed(page, "Home");
    await page.evaluate((selector) => { document.querySelector(selector).scrollTop = 158; }, MENU);
    expect(await pressed(page, "Home")).toMatchObject({ action: first, inside: true, scrollTop: 0 });

    // Shut, then open on the first row: in sight at the top; End from there.
    await page.keyboard.press("Escape");
    await settled(page);
    expect(await pressed(page, "ArrowDown")).toMatchObject({ action: first, inside: true, scrollTop: 0 });
    expect(await pressed(page, "End")).toMatchObject({ action: last, inside: true });
  }, { width: 320, height: 640 });
});
