import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { captureLayout, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { mountSelectSurface, openSelectDisclosures, SELECT_SOURCES, SELECT_SURFACES } from "./selectSurfaces.mjs";
import { settled } from "./chatMenuSeed.mjs";

it("covers every production module that renders a native select", () => {
  const root = fileURLToPath(new URL("../../src/", import.meta.url));
  const files = readdirSync(root, { recursive: true }).filter((file) => file.endsWith(".js"));
  const rendersSelect = (file) => readFileSync(`${root}${file}`, "utf8").split("\n")
    .some((line) => !/^\s*(?:\/\/|\*)/.test(line) && /<select\b|createElement\(["']select["']\)/.test(line));
  expect(files.filter(rendersSelect).sort()).toEqual(Object.keys(SELECT_SOURCES).sort());
});

async function selectStyles(page) {
  return page.evaluate(() => {
    const token = (name, property = "color") => {
      const probe = document.createElement("i");
      probe.style.position = "fixed";
      probe.style[property] = `var(${name})`;
      document.body.appendChild(probe);
      const value = getComputedStyle(probe)[property];
      probe.remove();
      return value;
    };
    const bodyFont = getComputedStyle(document.body).fontFamily;
    return [...document.querySelectorAll("select")].map((select) => {
      const style = getComputedStyle(select);
      const icon = getComputedStyle(select, "::picker-icon");
      const box = select.getBoundingClientRect();
      return {
        label: select.id || select.getAttribute("aria-label") || select.outerHTML.slice(0, 140),
        appearance: style.appearance, font: style.fontFamily, bodyFont,
        fontSize: parseFloat(style.fontSize),
        background: style.backgroundColor, expectedBackground: token("--panel"),
        color: style.color, expectedColor: token(select.disabled ? "--dim" : "--ink"),
        border: style.borderColor, expectedBorder: token("--line"), borderWidth: style.borderWidth,
        radius: style.borderRadius, expectedRadius: token("--select-radius", "borderRadius"),
        height: style.height, expectedHeight: token(select.classList.contains("mini") ? "--select-compact-height" : "--select-height", "height"),
        iconColor: icon.color, expectedIcon: token(select.disabled ? "--dim" : "--ink2"),
        iconContent: icon.content, iconPosition: icon.position, iconRight: icon.right,
        iconWidth: icon.width, iconBackground: icon.backgroundColor, iconImage: icon.backgroundImage,
        visible: box.width > 0 && box.height > 0, left: box.left, right: box.right, viewport: innerWidth,
      };
    });
  });
}

async function assertClosedSelects(page, surface) {
  await page.mouse.move(0, 0);
  await page.evaluate(() => document.activeElement?.blur());
  await settled(page);
  const controls = await selectStyles(page);
  expect(controls.length, surface).toBeGreaterThan(0);
  for (const control of controls) {
    expect(control.appearance, `${surface}: ${control.label}`).toBe("base-select");
    expect(control.font, control.label).toBe(control.bodyFont);
    if (control.viewport <= 720) expect(control.fontSize, control.label).toBeGreaterThanOrEqual(16);
    expect(control.background, control.label).toBe(control.expectedBackground);
    expect(control.color, control.label).toBe(control.expectedColor);
    expect(control.border, control.label).toBe(control.expectedBorder);
    expect(control.borderWidth, control.label).toBe("1px");
    expect(control.radius, control.label).toBe(control.expectedRadius);
    expect(control.height, control.label).toBe(control.expectedHeight);
    expect(control.iconColor, control.label).toBe(control.expectedIcon);
    expect(control.iconContent, control.label).toBe('""');
    expect(control.iconPosition, control.label).toBe("absolute");
    expect(control.iconRight, control.label).toBe("0px");
    expect(control.iconWidth, control.label).toBe("30px");
    expect(control.iconBackground, control.label).toBe(control.expectedBackground);
    expect(control.iconImage, control.label).toContain(control.expectedIcon);
    if (control.visible) {
      expect(control.left, control.label).toBeGreaterThanOrEqual(-1);
      expect(control.right, control.label).toBeLessThanOrEqual(control.viewport + 1);
    }
  }
  const overflow = await page.evaluate(() => ({ width: innerWidth, document: document.documentElement.scrollWidth }));
  expect(overflow.document, `${surface}: ${JSON.stringify(overflow)}`).toBeLessThanOrEqual(overflow.width + 1);
}

async function assertOpenPicker(page, surface) {
  const control = page.locator("select:not(:disabled):visible").first();
  if (await control.count() === 0) throw new Error(`${surface} has no usable select`);
  const optionsBefore = await control.locator("option").evaluateAll((options) => options.map((option) => ({ text: option.textContent, value: option.value, disabled: option.disabled })));
  const valueBefore = await control.inputValue();
  // A programmatic focus after opening a surface by pointer is not necessarily
  // focus-visible. Switch to keyboard modality before checking its focus ring.
  await page.keyboard.press("Tab");
  await control.focus();
  const focus = await control.evaluate((select) => {
    const style = getComputedStyle(select);
    const probe = document.createElement("i");
    probe.style.color = "var(--accent)";
    document.body.appendChild(probe);
    const answer = { border: style.borderColor, expectedBorder: getComputedStyle(probe).color, shadow: style.boxShadow };
    probe.remove();
    return answer;
  });
  await settled(page);
  expect(await control.evaluate((select) => getComputedStyle(select).borderColor), surface).toBe(focus.expectedBorder);
  expect(await control.evaluate((select) => getComputedStyle(select).boxShadow), surface).not.toBe("none");
  await page.keyboard.press("Space");
  await page.waitForFunction(() => !!document.querySelector("select:open"));
  const picker = await control.evaluate((select) => {
    const style = getComputedStyle(select, "::picker(select)");
    const probe = document.createElement("i");
    document.body.appendChild(probe);
    const color = (name) => { probe.style.color = `var(${name})`; return getComputedStyle(probe).color; };
    const root = getComputedStyle(document.documentElement);
    const checked = select.querySelector("option:checked");
    const options = [...select.querySelectorAll("option")].map((option) => {
      const box = option.getBoundingClientRect();
      return { left: box.left, right: box.right, height: box.height, background: getComputedStyle(option).backgroundColor };
    });
    const answer = { appearance: style.appearance, background: style.backgroundColor, expectedBackground: color("--panel"),
      border: style.borderColor, expectedBorder: color("--line"), radius: style.borderRadius,
      expectedRadius: root.getPropertyValue("--select-menu-radius").trim(), font: style.fontFamily,
      bodyFont: getComputedStyle(document.body).fontFamily, check: getComputedStyle(checked, "::checkmark").color,
      expectedCheck: color("--accent"), options, viewport: innerWidth };
    probe.remove();
    return answer;
  });
  expect(picker.appearance, surface).toBe("base-select");
  expect(picker.background, surface).toBe(picker.expectedBackground);
  expect(picker.border, surface).toBe(picker.expectedBorder);
  expect(picker.radius, surface).toBe(picker.expectedRadius);
  expect(picker.font, surface).toBe(picker.bodyFont);
  expect(picker.check, surface).toBe(picker.expectedCheck);
  for (const option of picker.options) {
    expect(option.left, surface).toBeGreaterThanOrEqual(-1);
    expect(option.right, surface).toBeLessThanOrEqual(picker.viewport + 1);
  }
  const option = control.locator("option:not(:disabled)").last();
  await option.hover();
  const hover = await option.evaluate((option) => {
    const probe = document.createElement("i");
    probe.style.backgroundColor = "var(--accent-soft)";
    document.body.appendChild(probe);
    const answer = { background: getComputedStyle(option).backgroundColor, expected: getComputedStyle(probe).backgroundColor };
    probe.remove();
    return answer;
  });
  expect(hover.background, surface).toBe(hover.expected);
  await captureLayout(page, `select-${surface.replaceAll(" ", "-")}-${await page.evaluate(() => document.documentElement.dataset.theme)}-open.png`);
  await control.locator("option:checked").evaluate((option) => option.focus());
  await page.keyboard.press("Enter");
  expect(await control.inputValue(), surface).toBe(valueBefore);
  expect(await control.locator("option").evaluateAll((options) => options.map((option) => ({ text: option.textContent, value: option.value, disabled: option.disabled })))).toEqual(optionsBefore);
  expect(await control.evaluate((select) => select === document.activeElement), surface).toBe(true);
  await page.evaluate(() => document.activeElement.blur());
}

for (const theme of ["light", "dark"]) {
  it(`styles every native select surface and its open picker without overflow at 320px in ${theme}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      const origin = new URL(page.url()).origin;
      const failures = [];
      for (const surface of SELECT_SURFACES) {
        try {
          await page.goto(`${origin}${basePath}src/styles.css`);
          await mountSelectSurface(page, basePath, surface.name, theme);
          // Disclosures must look right both before and after the reader opens them.
          await assertClosedSelects(page, surface.name);
          await openSelectDisclosures(page);
          await assertClosedSelects(page, surface.name);
          if (surface.name === "clear conversation") {
            const heights = await page.locator(".rail-reset-choice select, .rail-reset-choice .rail-harness-choice").evaluateAll((controls) => controls.map((control) => control.getBoundingClientRect().height));
            expect(new Set(heights), "Reset harness and field heights").toEqual(new Set([36]));
          }
          await captureLayout(page, `select-${surface.name.replaceAll(" ", "-")}-${theme}-closed.png`);
          await assertOpenPicker(page, surface.name);
        } catch (error) {
          failures.push(`${surface.name}: ${error.message}`);
        }
      }
      expect(failures).toEqual([]);
    }, { width: 320, height: 844 });
  }, 180_000);
}

it("keeps a real device mode selection keyboard operated and sends the same change", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountSelectSurface(page, basePath, "device settings", "dark");
    const mode = page.locator('#agentmode-claude');
    await mode.focus();
    await page.keyboard.press("Space");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__selectCalls.some((call) => call.method === "settings.set"));
    await page.waitForFunction(() => {
      const select = document.querySelector('#agentmode-claude');
      return select?.value === "tui" && !select.disabled;
    });
    expect(await page.evaluate(() => window.__selectCalls.filter((call) => call.method === "settings.set"))).toEqual([{ method: "settings.set", params: { agent_modes: { claude: "tui" } } }]);
    expect(await mode.inputValue()).toBe("tui");
    expect(await mode.isEnabled()).toBe(true);
  }, { width: 1280, height: 900 });
}, 30_000);

it("keeps the chevron visible with long selected device and model labels", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountSelectSurface(page, basePath, "local settings", "dark");
    const label = "A very long selected label with workspace, model, provider and capability details that must remain readable in the picker";
    await page.evaluate((label) => {
      for (const selector of ["#creationdev", "[data-harness-model]"]) {
        const select = document.querySelector(selector);
        select.options[select.selectedIndex].textContent = label;
      }
    }, label);
    await assertClosedSelects(page, "long selected labels");
    await page.locator("#creationdev").scrollIntoViewIfNeeded();
    await captureLayout(page, "select-long-label-closed.png");
    await assertOpenPicker(page, "long selected labels");
    expect(await page.locator("#creationdev option:checked").textContent()).toBe(label);
  }, { width: 320, height: 844 });
}, 30_000);

it("keeps the native fallback themed, keyboard usable, and 16px on a phone", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    // Exercise the same CSS fallback branch in Chromium without relying on a
    // second browser installation. Its native option popup remains native.
    await page.route("**/src/styles.css", async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, body: (await response.text()).replaceAll("(appearance:base-select)", "(appearance:select-test-fallback)") });
    });
    await mountLayout(page, '<main style="padding:16px"><label>Agent<select id="fallback"><option value="claude">Claude Code</option><option disabled value="unavailable">Unavailable</option><optgroup label="Other agents"><option value="codex">Codex</option></optgroup></select></label><label>Sort<select class="mini"><option>Latest changes</option><option>Alphabetical</option></select></label></main>', { basePath });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      await settled(page);
      const controls = await selectStyles(page);
      for (const control of controls) {
        expect(control.appearance).toBe("none");
        expect(control.font).toBe(control.bodyFont);
        expect(control.fontSize).toBeGreaterThanOrEqual(16);
        expect(control.background).toBe(control.expectedBackground);
        expect(control.border).toBe(control.expectedBorder);
        expect(control.radius).toBe(control.expectedRadius);
        expect(control.height).toBe(control.expectedHeight);
      }
      const arrow = await page.locator("#fallback").evaluate((select) => ({ image: getComputedStyle(select).backgroundImage, token: getComputedStyle(document.documentElement).getPropertyValue("--ink2") }));
      expect(arrow.image).toContain("linear-gradient");
      expect(arrow.image).not.toContain("url(");
      const expectedColor = await page.locator("#fallback").evaluate((select) => { select.style.color = "var(--ink2)"; const color = getComputedStyle(select).color; select.style.removeProperty("color"); return color; });
      expect(arrow.image).toContain(expectedColor);
    }
    const fallback = page.locator("#fallback");
    await fallback.focus();
    await page.keyboard.press("ArrowDown");
    expect(await fallback.inputValue()).toBe("codex");
    expect(await fallback.locator("optgroup").getAttribute("label")).toBe("Other agents");
    expect(await fallback.locator('option[value="unavailable"]').evaluate((option) => option.disabled)).toBe(true);
  }, { width: 320, height: 844 });
}, 30_000);
