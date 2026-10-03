import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { mountSelectSurface } from "./selectSurfaces.mjs";
import { settled } from "./chatMenuSeed.mjs";

async function openPicker(page, selector = "#position") {
  await page.locator(selector).focus();
  await page.keyboard.press("Space");
  await page.waitForFunction(() => !!document.querySelector("select:open"));
}

async function pickerBounds(page, selector = "#position") {
  return page.locator(selector).evaluate((select) => {
    const control = select.getBoundingClientRect();
    const picker = getComputedStyle(select, "::picker(select)");
    const options = [...select.options].map((option) => {
      const box = option.getBoundingClientRect();
      const style = getComputedStyle(option);
      return { left: box.left, right: box.right, top: box.top, bottom: box.bottom, height: box.height,
        whiteSpace: style.whiteSpace, overflow: style.overflow, textOverflow: style.textOverflow,
        oneLineHeight: parseFloat(style.lineHeight) + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)
          + parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth) };
    });
    return { control: { left: control.left, right: control.right, top: control.top, bottom: control.bottom, width: control.width },
      picker: { left: options[0].left - parseFloat(picker.paddingLeft) - parseFloat(picker.borderLeftWidth),
        right: options[0].right + parseFloat(picker.paddingRight) + parseFloat(picker.borderRightWidth),
        top: options[0].top - parseFloat(picker.paddingTop) - parseFloat(picker.borderTopWidth),
        bottom: options[0].top - parseFloat(picker.paddingTop) - parseFloat(picker.borderTopWidth) + parseFloat(picker.height) },
      overflowY: picker.overflowY, options, viewport: { width: innerWidth, height: innerHeight } };
  });
}

it("anchors a normal picker six pixels below and aligned with its control", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<select id="position"><option>First option</option><option>Second option</option></select>', {
      basePath, styles: '#position{position:absolute;left:120px;top:100px;width:180px}',
    });
    await openPicker(page);
    const bounds = await pickerBounds(page);
    expect(Math.abs(bounds.picker.left - bounds.control.left), JSON.stringify(bounds)).toBeLessThanOrEqual(1);
    expect(Math.abs(bounds.picker.top - bounds.control.bottom - 6), JSON.stringify(bounds)).toBeLessThanOrEqual(1);
    expect(bounds.picker.bottom).toBeLessThan(bounds.viewport.height - 12);
  }, { width: 800, height: 600 });
}, 30_000);

it.each([
  { edge: "left", left: 4, width: 180 },
  { edge: "right", left: 195, width: 117 },
])("keeps a picker twelve pixels inside the viewport near the $edge edge", async ({ left, width }) => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<select id="position"><option>One</option><option>Two</option></select>', {
      basePath, styles: `#position{position:absolute;left:${left}px;top:100px;width:${width}px}`,
    });
    await openPicker(page);
    const bounds = await pickerBounds(page);
    expect(bounds.picker.left, JSON.stringify(bounds)).toBeGreaterThanOrEqual(12);
    expect(bounds.picker.right, JSON.stringify(bounds)).toBeLessThanOrEqual(bounds.viewport.width - 12);
    expect(Math.abs(bounds.picker.top - bounds.control.bottom - 6), JSON.stringify(bounds)).toBeLessThanOrEqual(1);
  }, { width: 320, height: 600 });
}, 30_000);

it("scrolls a long picker within the space above or below its trigger on a short phone viewport", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    const options = Array.from({ length: 30 }, (_, index) => `<option>Option ${index + 1}</option>`).join("");
    await mountLayout(page, `<select id="position">${options}</select>`, {
      basePath, styles: '#position{position:absolute;left:80px;top:200px;width:180px}',
    });
    await openPicker(page);
    const bounds = await pickerBounds(page);
    const belowGap = bounds.picker.top - bounds.control.bottom;
    const aboveGap = bounds.control.top - bounds.picker.bottom;
    expect(Math.min(Math.abs(belowGap - 6), Math.abs(aboveGap - 6)), JSON.stringify(bounds)).toBeLessThanOrEqual(1);
    expect(bounds.picker.top, JSON.stringify(bounds)).toBeGreaterThanOrEqual(12);
    expect(bounds.picker.bottom, JSON.stringify(bounds)).toBeLessThanOrEqual(bounds.viewport.height - 12);
    expect(["auto", "scroll"], JSON.stringify(bounds)).toContain(bounds.overflowY);
    const contentHeight = bounds.options.reduce((height, option) => height + option.height, 0);
    expect(contentHeight).toBeGreaterThan(bounds.picker.bottom - bounds.picker.top);
    // The final option remains reachable through native keyboard scrolling.
    await page.keyboard.press("End");
    await page.keyboard.press("Enter");
    expect(await page.locator("#position").inputValue()).toBe("Option 30");
  }, { width: 320, height: 400 });
}, 30_000);

it("flips the picker above its control only when there is no room below", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<select id="position"><option>First option</option><option>Second option</option></select>', {
      basePath, styles: '#position{position:absolute;left:120px;bottom:24px;width:180px}',
    });
    await openPicker(page);
    const bounds = await pickerBounds(page);
    expect(Math.abs(bounds.picker.left - bounds.control.left), JSON.stringify(bounds)).toBeLessThanOrEqual(1);
    expect(Math.abs(bounds.control.top - bounds.picker.bottom - 6), JSON.stringify(bounds)).toBeLessThanOrEqual(1);
    expect(bounds.picker.top).toBeGreaterThanOrEqual(12);
  }, { width: 800, height: 600 });
}, 30_000);

for (const surface of [
  { name: "clear conversation", selector: "[data-clear-detail]" },
  { name: "local settings", selector: "#creationdev" },
]) {
  it(`places the actual ${surface.name} picker below the control when it fits`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountSelectSurface(page, basePath, surface.name, "light");
      await page.locator(surface.selector).scrollIntoViewIfNeeded();
      await openPicker(page, surface.selector);
      const bounds = await pickerBounds(page, surface.selector);
      const roomBelow = bounds.viewport.height - bounds.control.bottom - 6 - 12;
      const pickerHeight = bounds.picker.bottom - bounds.picker.top;
      expect(roomBelow, JSON.stringify(bounds)).toBeGreaterThanOrEqual(pickerHeight);
      expect(Math.abs(bounds.picker.left - bounds.control.left), JSON.stringify(bounds)).toBeLessThanOrEqual(1);
      expect(Math.abs(bounds.picker.top - bounds.control.bottom - 6), JSON.stringify(bounds)).toBeLessThanOrEqual(1);
    }, { width: 320, height: 900 });
  }, 30_000);
}

for (const width of [320, 1280]) {
  it(`lets the compact sort picker grow for single-line options and clips long labels at ${width}px`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, '<main id="fixture"></main>', { basePath, styles: '#fixture{padding:24px} .diffsort-select{width:126px}' });
      await loadBrowserModules(page, { sort: "src/core/diffSort.js" }, basePath);
      await page.evaluate(() => { document.querySelector("#fixture").innerHTML = window.__layoutModules.sort.diffSortHtml("alphabetical"); });
      await openPicker(page, ".diffsort-select");
      const short = await pickerBounds(page, ".diffsort-select");
      expect(short.picker.right - short.picker.left, JSON.stringify(short)).toBeGreaterThan(short.control.width);
      for (const option of short.options) {
        expect(option.whiteSpace).toBe("nowrap");
        expect(Math.abs(option.height - option.oneLineHeight), JSON.stringify(option)).toBeLessThanOrEqual(1);
      }
      await page.keyboard.press("Escape");
      const label = "A very long available model or project label with details that must not wrap into multiple rows ".repeat(6);
      await page.locator(".diffsort-select").evaluate((select, label) => { select.options[0].textContent = label; }, label);
      await openPicker(page, ".diffsort-select");
      const long = await pickerBounds(page, ".diffsort-select");
      expect(long.picker.left, JSON.stringify(long)).toBeGreaterThanOrEqual(12);
      expect(long.picker.right, JSON.stringify(long)).toBeLessThanOrEqual(long.viewport.width - 12);
      for (const option of long.options) {
        expect(option.whiteSpace).toBe("nowrap");
        expect(option.overflow).toBe("hidden");
        expect(option.textOverflow).toBe("ellipsis");
        expect(Math.abs(option.height - option.oneLineHeight), JSON.stringify(option)).toBeLessThanOrEqual(1);
      }
      expect(await page.locator(".diffsort-select option").first().textContent()).toBe(label);
    }, { width, height: 600 });
  }, 30_000);
}

it("matches select and input backgrounds in sheets and panels, with a short fade before the arrow", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    const label = "A long selected model label whose tail reaches the reserved arrow rail ".repeat(4);
    await mountLayout(page, `<main style="padding:16px"><div class="panel"><div class="field"><label for="panel-select">Model</label><select id="panel-select"><option>${label}</option></select><input id="panel-input" value="Model"></div></div><div class="sheet"><div class="field"><label for="sheet-select">Project</label><select id="sheet-select"><option>${label}</option></select><input id="sheet-input" value="Project"></div></div><label>Other<select id="default-select"><option>${label}</option></select></label></main>`, { basePath });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      await settled(page);
      const controls = await page.evaluate(() => [...document.querySelectorAll("select")].map((select) => {
        const style = getComputedStyle(select);
        const icon = getComputedStyle(select, "::picker-icon");
        const fade = getComputedStyle(select, "::after");
        const input = document.querySelector(`#${select.id.replace("select", "input")}`);
        const probe = document.createElement("i");
        probe.style.backgroundColor = `var(${input ? "--bg" : "--panel"})`;
        document.body.appendChild(probe);
        const expected = getComputedStyle(probe).backgroundColor;
        probe.remove();
        return { id: select.id, background: style.backgroundColor, input: input && getComputedStyle(input).backgroundColor,
          expected, iconBackground: icon.backgroundColor, fade: { content: fade.content, position: fade.position,
            right: fade.right, width: fade.width, image: fade.backgroundImage, pointerEvents: fade.pointerEvents } };
      }));
      for (const control of controls) {
        expect.soft(control.background, control.id).toBe(control.expected);
        if (control.input) expect(control.background, control.id).toBe(control.input);
        expect(control.iconBackground, control.id).toBe(control.expected);
        expect(control.fade.content, control.id).toBe('""');
        expect(control.fade.position, control.id).toBe("absolute");
        expect(control.fade.right, control.id).toBe("30px");
        expect(control.fade.width, control.id).toBe("16px");
        expect(control.fade.pointerEvents, control.id).toBe("none");
        expect(control.fade.image, control.id).toContain("linear-gradient");
        expect(control.fade.image, control.id).toContain(control.expected);
      }
    }
  }, { width: 320, height: 844 });
}, 30_000);
