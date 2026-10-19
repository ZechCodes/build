import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

async function appearance(control) {
  return control.evaluate((element) => {
    const style = getComputedStyle(element);
    const { x, y, width, height } = element.getBoundingClientRect();
    const probe = document.createElement("span");
    probe.style.color = "var(--accent)";
    element.append(probe);
    const accent = getComputedStyle(probe).color;
    probe.remove();
    return { colour: style.color, background: style.backgroundColor, shadow: style.boxShadow,
      outline: style.outlineStyle, outlineWidth: parseFloat(style.outlineWidth), outlineColour: style.outlineColor,
      accent, bounds: { x, y, width, height },
      icons: [...element.querySelectorAll("svg, .tb-caret")].map((icon) => getComputedStyle(icon).color) };
  });
}

for (const width of [390, 1280]) for (const theme of ["light", "dark"]) {
  it(`toolbar hover changes only ink and preserves keyboard focus ${width}px ${theme}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      for (const kind of ["workspace", "projectTask", "branch"]) {
        await mountNavigationBreadcrumbs(page, basePath, { theme, kind });
        if (kind === "branch") {
          await page.addScriptTag({ type: "module", content: `import { mountSplitButton } from '${basePath}src/core/splitButton.js'; window.__navigationSplit = mountSplitButton;` });
          await page.waitForFunction(() => !!window.__navigationSplit);
          await page.evaluate(() => {
            window.__navigationSplit(document.querySelector("#tb-verb"), {
              options: [{ id: "done", label: "Done" }, { id: "keep", label: "Keep branch" }], variant: "mini", run() {},
            });
          });
        }
        const controls = page.locator('.toolbar button:not(:disabled), .toolbar a[href]');
        for (let index = 0; index < await controls.count(); index++) {
          const control = controls.nth(index);
          await page.mouse.move(0, 0);
          const before = await appearance(control);
          await control.hover();
          await page.waitForTimeout(200);
          const hovered = await appearance(control);
          expect(hovered.colour).toBe(hovered.accent);
          expect(hovered.background).toBe("rgba(0, 0, 0, 0)");
          expect(hovered.background).toBe(before.background);
          expect(hovered.shadow).toBe("none");
          expect(hovered.icons.every((colour) => colour === hovered.accent)).toBe(true);
          expect(hovered.bounds).toEqual(before.bounds);
          await page.mouse.move(0, 0);
          await page.keyboard.press("Tab");
          await control.focus();
          expect(await control.evaluate((element) => element.matches(":focus-visible"))).toBe(true);
          const focused = await appearance(control);
          expect(focused.outline).toBe("solid");
          expect(focused.outlineWidth).toBeGreaterThanOrEqual(2);
          expect(focused.outlineColour).toBe(focused.accent);
          expect(focused.background).toBe("rgba(0, 0, 0, 0)");
          expect(focused.bounds).toEqual(before.bounds);
        }
      }
    }, { width, height: 844 });
  }, 45_000);
}

it("left chevron stays close to the project with a compact, full-height phone hit target", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountNavigationBreadcrumbs(page, basePath);
    const switcher = page.getByRole("button", { name: "Switch project or workspace", exact: true });
    const icon = switcher.locator("svg.lucide-chevron-left");
    expect(await icon.count()).toBe(1);
    expect(await switcher.locator('[aria-hidden="true"]').count()).toBe(1);
    const target = await switcher.boundingBox();
    expect(target.width).toBe(32);
    expect(target.height).toBeGreaterThanOrEqual(44);
    const glyph = await icon.boundingBox();
    const project = await page.locator("[data-project-return]").boundingBox();
    expect(project.x - (glyph.x + glyph.width)).toBeLessThanOrEqual(4);
    expect(target.x + target.width).toBeLessThanOrEqual(project.x);
    await switcher.tap();
    await page.locator(".tb-filter").waitFor();
    expect(await switcher.getAttribute("aria-expanded")).toBe("true");
    await page.keyboard.press("Escape");
    expect(await switcher.evaluate((element) => document.activeElement === element)).toBe(true);
  }, { width: 390, height: 844, hasTouch: true });
}, 30_000);
