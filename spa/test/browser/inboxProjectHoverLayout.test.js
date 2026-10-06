import { expect, it } from "vitest";
import { captureLayout, withLayoutPage } from "./layoutHarness.mjs";
import { mountStatusDotInbox } from "./inboxStatusDotSeed.mjs";

async function headLayout(head) {
  return head.evaluate((element) => {
    const box = (node) => {
      const { x, y, width, height } = node.getBoundingClientRect();
      return { x, y, width, height };
    };
    const name = element.querySelector(".inbox-project-name");
    return {
      box: box(element), background: getComputedStyle(element).backgroundColor,
      name: box(name), nameBackground: getComputedStyle(name).backgroundColor,
      dot: box(element.querySelector(":scope > .inbox-status-dot")),
      controls: [...element.querySelectorAll(".inbox-fold, .inbox-project-create, .inbox-project-hide")].map(box),
      deviceOpacity: getComputedStyle(element.querySelector(".inbox-project-device")).opacity,
      actionsOpacity: getComputedStyle(element.querySelector(".inbox-project-actions")).opacity,
    };
  });
}

for (const { label, width, height } of [
  { label: "desktop", width: 1280, height: 800 },
  { label: "mobile", width: 390, height: 760 },
]) {
  for (const theme of ["light", "dark"]) {
    it(`fills the whole project head on hover on ${label} in ${theme}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await page.emulateMedia({ reducedMotion: "reduce" });
        await mountStatusDotInbox(page, basePath, { grouped: true });
        await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
        const block = page.locator(".inbox-project");
        const head = block.locator(".inbox-project-head");
        const name = head.locator(".inbox-project-name");
        const row = block.locator(".inbox-entry").first();
        const quietRow = block.locator(".inbox-quiet");
        const fills = await block.evaluate((element) => {
          const probe = document.createElement("span");
          probe.style.transition = "none";
          element.appendChild(probe);
          const colors = ["--row-hover", "--accent-soft"].map((token) => {
            probe.style.background = `var(${token})`;
            return getComputedStyle(probe).backgroundColor;
          });
          probe.remove();
          return colors;
        });
        for (const active of [false, true]) {
          await block.evaluate((element, active) => { element.classList.toggle("active", active); }, active);
          await row.evaluate((element, active) => { element.classList.toggle("active", active); }, active);
          await page.mouse.move(width - 1, height - 1);
          await page.waitForFunction(() => getComputedStyle(document.querySelector(".inbox-project-actions")).opacity === "0");
          const before = await headLayout(head);
          expect(before.deviceOpacity).toBe("0.72");
          await row.hover();
          await page.waitForFunction(({ key, background }) =>
            getComputedStyle(document.querySelector(`[data-key="${key}"]`)).backgroundColor === background,
          { key: await row.getAttribute("data-key"), background: fills[active ? 1 : 0] });
          const rowFill = await row.evaluate((element) => {
            const { left, right } = element.getBoundingClientRect();
            return { left, right, background: getComputedStyle(element).backgroundColor };
          });
          expect(rowFill.background).toBe(fills[active ? 1 : 0]);
          const quietHeight = (await quietRow.boundingBox()).height;
          for (const target of ["name", "left gutter", "right gutter"]) {
            if (target === "name") await name.hover();
            else await page.mouse.move(target === "left gutter" ? rowFill.left + 1 : rowFill.right - 1,
              before.box.y + before.box.height / 2);
            const hovered = await headLayout(head);
            const state = `${active ? "active" : "inactive"}, ${target}`;
            await captureLayout(page, `inbox-project-hover-${label}-${theme}-${active ? "active" : "inactive"}-${target.replaceAll(" ", "-")}.png`);
            expect.soft(await head.evaluate((element) => element.matches(":hover")), state).toBe(true);
            expect.soft(hovered.background, state).toBe(rowFill.background);
            expect.soft(hovered.box.x, state).toBe(rowFill.left);
            expect.soft(hovered.box.x + hovered.box.width, state).toBe(rowFill.right);
            expect.soft(hovered.box.height, state).toBeGreaterThanOrEqual(quietHeight);
            expect.soft(hovered.nameBackground, state).toBe("rgba(0, 0, 0, 0)");
            expect.soft(hovered.name, state).toEqual(before.name);
            expect.soft(hovered.dot, state).toEqual(before.dot);
            expect.soft(hovered.controls, state).toEqual(before.controls);
          }
          await name.hover();
          await page.waitForFunction(() => getComputedStyle(document.querySelector(".inbox-project-actions")).opacity === "1");
          const hovered = await headLayout(head);
          expect(hovered.deviceOpacity).toBe("0");
          expect(hovered.actionsOpacity).toBe("1");
        }
        await page.mouse.move(width - 1, height - 1);
        await page.keyboard.press("Tab");
        await name.focus();
        expect(await name.evaluate((element) => element.tagName)).toBe("BUTTON");
        expect(await name.evaluate((element) => element.matches(":focus-visible"))).toBe(true);
        expect(await name.evaluate((element) => getComputedStyle(element).outlineStyle)).not.toBe("none");
        expect(await name.evaluate((element) => parseFloat(getComputedStyle(element).outlineWidth))).toBeGreaterThan(0);
      }, { width, height });
    }, 60_000);
  }
}
