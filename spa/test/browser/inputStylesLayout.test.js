import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { mountInputSurface, INPUT_SURFACES, INPUT_SOURCES, INPUT_NONVISUAL_SOURCES, INPUT_TOGGLE_SOURCES, INPUT_RANGE_SOURCES, INPUT_FILE_SOURCES, INPUT_RADIO_SOURCES } from "./inputSurfaces.mjs";
import { settled } from "./chatMenuSeed.mjs";

it("inventories every input/textarea source and explicitly lists separate checkbox, radio, range and file controls", () => {
  const root = fileURLToPath(new URL("../../src/", import.meta.url));
  const sources = readdirSync(root, { recursive: true }).filter((file) => file.endsWith(".js"));
  const renders = (pattern) => sources.filter((file) => readFileSync(`${root}${file}`, "utf8").split("\n")
    .some((line) => !/^\s*(?:\/\/|\*)/.test(line) && pattern.test(line))).sort();
  const inventory = { ...INPUT_SOURCES, ...INPUT_NONVISUAL_SOURCES, ...INPUT_TOGGLE_SOURCES, ...INPUT_RANGE_SOURCES, ...INPUT_FILE_SOURCES, ...INPUT_RADIO_SOURCES };
  expect(renders(/<(?:input|textarea)\b|createElement\(["'](?:input|textarea)["']\)/)).toEqual(Object.keys(inventory).sort());
  expect(renders(/<input\b[^>]*type=["']checkbox["']/)).toEqual(Object.keys(INPUT_TOGGLE_SOURCES).sort());
  expect(renders(/<input\b[^>]*type=["']radio["']/)).toEqual(Object.keys(INPUT_RADIO_SOURCES).sort());
  expect(renders(/<input\b[^>]*type=["']range["']/)).toEqual(Object.keys(INPUT_RANGE_SOURCES).sort());
  expect(renders(/<input\b[^>]*type=["']file["']/)).toEqual(Object.keys(INPUT_FILE_SOURCES).sort());
});

const excludedTypes = ["checkbox", "radio", "file", "hidden", "button", "submit", "reset", "range", "color", "image"];

export async function textControlStyles(page) {
  return page.evaluate((excluded) => {
    const token = (name, property = "color") => {
      const probe = document.createElement("i");
      probe.style.position = "fixed";
      probe.style[property] = `var(${name})`;
      document.body.appendChild(probe);
      const value = getComputedStyle(probe)[property];
      probe.remove();
      return value;
    };
    // Enumerate the document, including fields in closed disclosures. The only
    // nonvisual textarea is the composer's aria-hidden measurement mirror.
    return [...document.querySelectorAll("input, textarea")].map((field) => {
      if (field.tagName === "INPUT" && excluded.includes(field.type)) return { excluded: field.type };
      if (field.tagName === "TEXTAREA" && field.getAttribute("aria-hidden") === "true" && field.readOnly && field.tabIndex === -1) return { excluded: "measurement mirror" };
      const style = getComputedStyle(field);
      const rect = field.getBoundingClientRect();
      const rail = field.matches(".rail-composer .composer textarea");
      const editor = field.matches(".file-editor");
      return {
        label: field.id || field.outerHTML.slice(0, 160), tag: field.tagName,
        font: style.fontFamily, expectedFont: token(editor ? "--mono" : "--sans", "fontFamily"), fontSize: parseFloat(style.fontSize),
        background: style.backgroundColor, expectedBackground: rail ? "rgba(0, 0, 0, 0)" : token(editor ? "--code-bg" : field.matches(".panel .field :is(input, textarea), .sheet :is(input, textarea), .composer textarea") ? "--bg" : "--panel"),
        border: style.borderColor, expectedBorder: token("--line"), borderWidth: style.borderWidth,
        radius: style.borderRadius, expectedRadius: token("--select-radius", "borderRadius"),
        height: style.height, expectedHeight: token(field.classList.contains("mini") ? "--select-compact-height" : "--select-height", "height"),
        compact: field.classList.contains("mini"), compactContext: field.matches(".tb-filter, .fmenu-search, .reroute-branch input, .workspace-refsearch"),
        color: style.color, expectedColor: token(field.disabled ? "--dim" : field.readOnly ? "--ink2" : "--ink"),
        opacity: style.opacity, disabled: field.disabled, rail, editor, resize: style.resize, minHeight: style.minHeight, maxHeight: style.maxHeight,
        visible: rect.width > 0 && rect.height > 0, left: rect.left, right: rect.right, viewport: innerWidth,
      };
    });
  }, excludedTypes);
}

async function assertTextControls(page, surface) {
  await page.mouse.move(0, 0);
  await page.evaluate(() => document.activeElement?.blur());
  await settled(page);
  const all = await textControlStyles(page);
  const fields = all.filter((field) => !field.excluded);
  expect(fields.length, `${surface}: no text fields mounted`).toBeGreaterThan(0);
  for (const field of fields) {
    const label = `${surface}: ${field.label}`;
    expect(field.font, label).toBe(field.expectedFont);
    expect(field.background, label).toBe(field.expectedBackground);
    expect(field.color, label).toBe(field.expectedColor);
    if (field.viewport <= 720) expect(field.fontSize, label).toBeGreaterThanOrEqual(16);
    if (!field.rail && !field.editor) {
      expect(field.borderWidth, label).toBe("1px");
      expect(field.border, label).toBe(field.expectedBorder);
      expect(field.radius, label).toBe(field.expectedRadius);
    }
    if (field.rail || field.editor) {
      expect(field.borderWidth, label).toBe("0px");
      expect(field.radius, label).toBe(field.editor ? "0px" : "12px 12px 0px 0px");
      expect(field.resize, label).toBe("none");
      if (field.rail) {
        expect(field.minHeight, label).toBe("36px");
        expect(parseFloat(field.maxHeight), label).toBe(await page.evaluate(() => innerHeight * 0.4));
      }
    }
    if (field.tag === "INPUT") {
      expect(field.height, label).toBe(field.expectedHeight);
      expect(["28px", "36px"], label).toContain(field.height);
      if (field.compactContext) expect(field.compact, label).toBe(true);
      expect(field.fontSize, label).toBe(field.viewport <= 720 ? 16 : field.compact ? 12 : 14);
    }
    if (field.disabled) expect(field.opacity, label).toBe("0.7");
    if (field.visible) {
      expect(field.left, label).toBeGreaterThanOrEqual(-1);
      expect(field.right, label).toBeLessThanOrEqual(field.viewport + 1);
    }
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth), surface).toBeLessThanOrEqual(await page.evaluate(() => innerWidth + 1));
}

for (const width of [320, 1280]) {
  for (const theme of ["light", "dark"]) {
    it(`matches every mounted input and textarea to shared field tokens at ${width}px in ${theme}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        const origin = new URL(page.url()).origin;
        const failures = [];
        for (const surface of INPUT_SURFACES) {
          try {
            await page.goto(`${origin}${basePath}src/styles.css`);
            await mountInputSurface(page, basePath, surface.name, theme);
            await page.waitForSelector(surface.selectors[0], { state: "visible" });
            await assertTextControls(page, surface.name);
            await page.evaluate(() => window.__inputDispose?.());
          } catch (error) { failures.push(`${surface.name}: ${error.message}`); }
        }
        expect(failures).toEqual([]);
      }, { width, height: 900 });
    }, 240_000);
  }
}

it("styles text-like types and focus/disabled states while retaining native editing and selection", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    // Compare settled paint, independently of the field transition clock.
    await page.emulateMedia({ reducedMotion: "reduce" });
    const types = ["text", "search", "url", "email", "password", "tel", "number"];
    await mountLayout(page, `<main style="padding:16px"><select><option>Main</option></select>${types.map((type) => `<input id="field-${type}" type="${type}" value="${type === "number" ? "2" : "main"}">`).join("")}<textarea id="prose">Two lines\nof prose</textarea><textarea id="readonly-prose" readonly>Saved prose</textarea><textarea id="disabled-prose" disabled>Paused prose</textarea><input id="readonly" readonly value="Saved"><input id="disabled" disabled value="Paused"><input id="toggle" type="checkbox"><input type="radio"><input type="range"><input type="file" hidden></main>`, { basePath });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      await assertTextControls(page, "all native types");
      const paintOf = (field) => {
        const style = getComputedStyle(field);
        return { border: style.borderColor, background: style.backgroundColor, shadow: style.boxShadow, outline: style.outlineStyle };
      };
      for (const selector of ["#readonly", "#readonly-prose"]) {
        await page.mouse.move(0, 0);
        const beforeHover = await page.locator(selector).evaluate(paintOf);
        await page.locator(selector).hover();
        expect(await page.locator(selector).evaluate(paintOf)).toEqual(beforeHover);
        expect(await page.locator(selector).evaluate((field) => getComputedStyle(field).cursor)).toBe("default");
      }
      expect(await page.locator("#disabled").evaluate((field) => getComputedStyle(field).cursor)).toBe("not-allowed");
      expect(await page.locator("#toggle").evaluate((field) => getComputedStyle(field).cursor)).not.toBe("default");
      await page.locator("select").hover();
      const hover = await page.locator("select").evaluate(paintOf);
      await page.locator("#field-text").hover();
      expect(await page.locator("#field-text").evaluate(paintOf)).toEqual(hover);
      await page.mouse.move(0, 0);
      await page.keyboard.press("Tab");
      await page.locator("select").focus();
      const focus = await page.locator("select").evaluate(paintOf);
      expect(focus.shadow).not.toBe("none");
      for (const selector of ["#field-text", "#prose"]) {
        await page.locator(selector).focus();
        expect(await page.locator(selector).evaluate(paintOf)).toEqual(focus);
      }
      expect(await page.locator("#disabled").isEnabled()).toBe(false);
      expect(await page.locator("#readonly").getAttribute("readonly")).toBe("");
    }
    await page.locator("#field-text").fill("feature/shared-inputs");
    await page.locator("#field-text").press("Home");
    await page.locator("#field-text").press("Shift+End");
    expect(await page.locator("#field-text").evaluate((field) => field.value.slice(field.selectionStart, field.selectionEnd))).toBe("feature/shared-inputs");
    expect(await page.locator("#prose").inputValue()).toBe("Two lines\nof prose");
    expect((await textControlStyles(page)).filter((field) => field.excluded).map((field) => field.excluded)).toEqual(["checkbox", "radio", "range", "file"]);
  }, { width: 320, height: 900 });
}, 30_000);

it("keeps production rail composer growth, capping, shrinkage and parent focus treatment", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountInputSurface(page, basePath, "new conversation", "dark");
    const input = page.locator(".rail-composer textarea");
    await input.fill("Short reply");
    await settled(page);
    const short = await input.evaluate((field) => field.getBoundingClientRect().height);
    await input.fill(Array.from({ length: 100 }, (_, index) => `A long line of text in the production composer, number ${index}`).join("\n"));
    await settled(page);
    const tall = await input.evaluate((field) => ({ height: field.getBoundingClientRect().height, scroll: field.scrollHeight, client: field.clientHeight, cap: parseFloat(getComputedStyle(field).maxHeight) }));
    expect(tall.height).toBeGreaterThan(short);
    expect(tall.height).toBe(tall.cap);
    expect(tall.scroll).toBeGreaterThan(tall.client);
    await input.fill("Short reply");
    await settled(page);
    expect(await input.evaluate((field) => field.getBoundingClientRect().height)).toBe(short);
    expect(await input.inputValue()).toBe("Short reply");
    expect(await input.evaluate((field) => getComputedStyle(field).boxShadow)).toBe("none");
    expect(await input.evaluate((field) => getComputedStyle(field.closest(".composer")).boxShadow)).not.toBe("none");
  }, { width: 320, height: 900 });
}, 30_000);
