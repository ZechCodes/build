// Capture the conversation head's ⋮ menu for #124, open, in both themes: at
// desktop and phone width, and at the narrowest phone width with four-digit
// counts on the surface rows. The production rail is mounted on a seeded
// workspace whose agent has surfaces and whose bridge carries compaction
// (chatMenuSeed.mjs).
// Run from spa/: node test/browser/captureChatMenu.mjs before|after [desktop|mobile|narrow]
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { withLayoutPage } from "./layoutHarness.mjs";
import { openMenuOn, settled } from "./chatMenuSeed.mjs";

const phase = process.argv[2] || "after";
const only = process.argv[3] || "";
const output = fileURLToPath(new URL("../../../design/chat-menu/", import.meta.url));
await mkdir(output, { recursive: true });

/** Each viewport: its size, whether a 2x close-up of the open menu is taken,
 *  the keys a keyboard close-up presses after opening from the opener with
 *  ArrowDown (desktop walks down two rows; the narrow phone, where the menu
 *  scrolls inside itself, jumps to the last row with End), and whether the
 *  surface rows carry four-digit counts (the wrap check). */
const VIEWPORTS = [
  ["desktop", { width: 1320, height: 850 }, { closeup: true, keyboard: ["ArrowDown", "ArrowDown"] }],
  ["mobile", { width: 390, height: 844 }, {}],
  ["narrow", { width: 320, height: 640 }, { closeup: true, keyboard: ["End"], bigCounts: true }],
];
const THEMES = ["light", "dark"];

/** The head and the open menu, with a margin, in page coordinates. */
const menuBox = (page) => page.evaluate(() => {
  const menu = document.querySelector(".rail-surface-menu .splitmenu").getBoundingClientRect();
  const head = document.querySelector(".rail-head").getBoundingClientRect();
  const x = Math.max(0, Math.min(menu.left, head.left) - 12);
  const y = Math.max(0, Math.min(menu.top, head.top) - 12);
  return { x, y, width: Math.min(window.innerWidth, Math.max(menu.right, head.right) + 12) - x, height: Math.min(window.innerHeight, Math.max(menu.bottom, head.bottom) + 12) - y };
});

for (const [label, viewport, { closeup = false, keyboard = null, bigCounts = false }] of VIEWPORTS) {
  if (only && label !== only) continue;
  for (const theme of THEMES) {
    await withLayoutPage(async ({ page, basePath }) => {
      const seed = { theme, bigCounts };
      await openMenuOn(page, basePath, label, seed);
      await page.screenshot({ path: `${output}${phase}-${theme}-${label}.png` });
      if (!closeup) return;
      // A close-up of the head and the open menu, so the rows read at review size.
      const box = await menuBox(page);
      const sharp = await page.context().browser().newPage({ viewport, deviceScaleFactor: 2 });
      await sharp.goto(page.url());
      await openMenuOn(sharp, basePath, label, seed);
      await sharp.screenshot({ path: `${output}${phase}-${theme}-${label}-closeup.png`, clip: box });
      if (keyboard && phase === "after") {
        // The keyboard's row: shut, park the pointer so no row wears its
        // hover, reopen from the opener with the arrows, press the
        // viewport's keys, and show where focus is.
        await sharp.keyboard.press("Escape");
        await settled(sharp);
        await sharp.mouse.move(0, 0);
        await sharp.locator(".rail-surface-menu .caret").focus();
        for (const key of ["ArrowDown", ...keyboard]) {
          await sharp.keyboard.press(key);
          await settled(sharp);
        }
        await sharp.screenshot({ path: `${output}${phase}-${theme}-${label}-keyboard.png`, clip: box });
      }
      await sharp.close();
    }, viewport);
  }
}
console.log(`wrote ${output}`);
