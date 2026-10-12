import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { COMPOSER_VARIANTS, THREE_LINES, mountChangesComposers, settleChangesTheme } from "./changesComposerSeed.mjs";

async function inspectControl(page, id) {
  return page.locator(`#${id}`).evaluate((host) => {
    const field = host.querySelector("textarea").getBoundingClientRect();
    const button = host.querySelector(".btn:not(.caret)");
    const style = getComputedStyle(button);
    const rect = button.getBoundingClientRect();
    const caret = host.querySelector(".caret")?.getBoundingClientRect();
    const send = document.querySelector(".composer-send").getBoundingClientRect();
    return { height: rect.height, bottom: rect.bottom, fieldBottom: field.bottom, fieldHeight: field.height,
      right: host.getBoundingClientRect().right, viewport: innerWidth,
      sendHeight: send.height, fontSize: style.fontSize,
      toolbarFontSize: getComputedStyle(document.querySelector("#verbar .btn")).fontSize,
      background: style.backgroundColor, color: style.color,
      toolbarColor: getComputedStyle(document.querySelector("#verbar .btn")).color,
      caretHeight: caret?.height, caretBottom: caret?.bottom };
  });
}

for (const width of [390, 1280]) {
  it(`keeps changes verbs quiet, compact and bottom aligned at ${width}px (#521)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountChangesComposers(page, basePath);
      for (const theme of ["light", "dark"]) {
        await settleChangesTheme(page, theme);
        for (const { id } of COMPOSER_VARIANTS) {
          await page.locator(`#${id} textarea`).fill("");
          const empty = await inspectControl(page, id);
          expect(empty.height).toBeLessThanOrEqual(empty.sendHeight);
          expect(empty.height).toBeLessThanOrEqual(30);
          expect(empty.fontSize).toBe(empty.toolbarFontSize);
          expect(empty.background).toBe("rgba(0, 0, 0, 0)");
          expect(empty.color).toBe(empty.toolbarColor);
          expect(empty.right).toBeLessThanOrEqual(empty.viewport);
          await page.locator(`#${id} textarea`).fill(THREE_LINES);
          const grown = await inspectControl(page, id);
          expect(grown.fieldHeight).toBeGreaterThan(empty.fieldHeight);
          expect(grown.height).toBe(empty.height);
          expect(grown.bottom).toBe(grown.fieldBottom);
          if (id === "split") {
            expect(grown.caretHeight).toBe(grown.height);
            expect(grown.caretBottom).toBe(grown.bottom);
          }
        }
        const shared = await page.locator("#shared .btn:not(.caret)").evaluate((button) => ({
          height: button.getBoundingClientRect().height, background: getComputedStyle(button).backgroundColor,
        }));
        expect(shared.height).toBeGreaterThan(30);
        expect(shared.background).not.toBe("rgba(0, 0, 0, 0)");
      }
    }, { width, height: 844, hasTouch: width === 390 });
  }, 60_000);
}

it("keeps changes verbs, split menu and keyboard focus working (#521)", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountChangesComposers(page, basePath);
    for (const id of ["comment", "commit"]) {
      await page.locator(`#${id} textarea`).fill("Review message");
      await page.keyboard.press("Tab");
      const button = page.locator(`#${id} .btn:not(.caret)`);
      expect(await button.evaluate((element) => element === document.activeElement)).toBe(true);
      expect(await button.evaluate((element) => getComputedStyle(element).outlineStyle)).toBe("solid");
      await page.keyboard.press("Enter");
    }
    await page.locator("#split textarea").fill("Commit message");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    expect(await page.locator("#split .caret").evaluate((element) => element === document.activeElement)).toBe(true);
    await page.keyboard.press("ArrowDown");
    expect(await page.locator("#split .caret").getAttribute("aria-expanded")).toBe("true");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__runs.length === 3);
    expect(await page.evaluate(() => window.__runs)).toEqual([
      { variant: "comment", verb: "comment", text: "Review message" },
      { variant: "commit", verb: "commit", text: "Review message" },
      { variant: "split", verb: "commit", text: "Commit message" },
    ]);
  });
}, 60_000);
