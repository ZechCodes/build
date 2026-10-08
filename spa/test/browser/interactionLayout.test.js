import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";
import { COMMENT_TEXT, dragInteractionText, mountInteractionFixture } from "./interactionFixture.mjs";

const chrome = [".tb-legacy-item .tb-name", ".task-page-state", ".task-rail-section h2", ".tabs .t", ".task-row", ".task-card", ".task-column-head", ".task-assign", ".task-page-labels .task-label", ".file td.ln", ".markdown label"];
const content = [".task-page-title", ".task-page-body", ".task-comment-body", ".file td.code", "#task-labels", "#task-comment", "#interaction-readonly", "#interaction-editor"];

for (const theme of ["light", "dark"]) {
  it(`keeps production chrome unselectable and content copyable in ${theme}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountInteractionFixture(page, basePath, { theme, extras: true });
      for (const [selectors, expected] of [[chrome, "none"], [content, "text"]]) {
        for (const selector of selectors) {
          const target = page.locator(selector).first();
          expect(await target.count(), selector).toBe(1);
          expect(await target.evaluate((node) => getComputedStyle(node).userSelect), selector).toBe(expected);
        }
      }
      expect(await page.locator(".bridge-update-command").evaluate((node) => getComputedStyle(node).userSelect)).toBe("all");
      const fields = await page.locator("input, textarea, select").evaluateAll((nodes) => nodes.map((node) => ({ type: node.tagName === "INPUT" ? node.type : node.tagName.toLowerCase(), select: getComputedStyle(node).userSelect })));
      expect(fields.map((field) => field.type)).toEqual(expect.arrayContaining(["text", "checkbox", "hidden", "file", "textarea", "select"]));
      for (const field of fields) expect(field.select, field.type).toBe("text");
      const cursors = [[".tb-sel", "pointer"], [".tabs .t", "pointer"], [".task-row", "default"], [".task-row-open", "pointer"], [".task-card", "grab"], [".task-page-state", "default"], [".task-rail-section h2", "default"], [".task-composer button:disabled", "default"]];
      for (const [selector, cursor] of cursors) expect(await page.locator(selector).first().evaluate((node) => getComputedStyle(node).cursor), selector).toBe(cursor);
      expect(await dragInteractionText(page, ".tb-legacy-item .tb-name", { clickCount: 3 })).toBe("");
      await captureLayout(page, `interaction-header-${theme}.png`);
      expect(await dragInteractionText(page, ".task-comment-body p")).toBe(COMMENT_TEXT);
      await captureLayout(page, `interaction-comment-${theme}.png`);
      expect(await dragInteractionText(page, ".task-page-title")).toBe("Make Build feel like an app");
      expect(await dragInteractionText(page, ".file td.ln:not(:empty)")).toBe("");
      expect((await dragInteractionText(page, ".file td.code")).length).toBeGreaterThan(0);
    }, { width: 1280, height: 900 });
  }, 60_000);
}

it("copies markdown tables while a button inside a comment stays unselectable", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountInteractionFixture(page, basePath, { extras: true });
    expect(await dragInteractionText(page, ".task-page-body th")).toBe("Area");
    expect(await dragInteractionText(page, ".task-comment-body [data-comment-action]")).toBe("");
    expect(await page.locator(".task-page-body th").first().evaluate((node) => getComputedStyle(node).userSelect)).toBe("text");
    expect(await page.locator("[data-comment-action]").evaluate((node) => getComputedStyle(node).userSelect)).toBe("none");
  });
}, 60_000);

it("copies values and errors nested inside navigation chrome", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountInteractionFixture(page, basePath, { extras: true });
    expect(await dragInteractionText(page, ".toolbar .v")).toBe("Copy this diagnostic value");
    expect(await dragInteractionText(page, "[data-error-row] .error")).toBe("Upload refused");
    expect(await page.locator(".toolbar .v span").evaluate((node) => getComputedStyle(node).userSelect)).toBe("text");
    expect(await page.locator("[data-error-row] .error span").evaluate((node) => getComputedStyle(node).userSelect)).toBe("text");
  });
}, 60_000);

it("keeps native editing, pointer feedback and keyboard focus while disabled controls stay quiet", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountInteractionFixture(page, basePath);
    const button = page.locator("[data-task-state]");
    const paint = (node) => {
      const style = getComputedStyle(node);
      return { background: style.backgroundColor, border: style.borderColor, transform: style.transform, filter: style.filter, outline: style.outlineStyle, shadow: style.boxShadow };
    };
    await page.mouse.move(0, 0);
    const idle = await button.evaluate(paint);
    await button.hover();
    const hovered = await button.evaluate(paint);
    expect(hovered).not.toEqual(idle);
    await page.mouse.down();
    expect(await button.evaluate((node) => node.matches(":active"))).toBe(true);
    const pressed = await button.evaluate(paint);
    expect(pressed).not.toEqual(hovered);
    await page.mouse.up();
    await page.mouse.move(0, 0);
    await page.keyboard.press("Tab");
    await button.focus();
    expect(await button.evaluate((node) => node.matches(":focus-visible"))).toBe(true);
    expect(await button.evaluate((node) => getComputedStyle(node).outlineStyle)).not.toBe("none");
    const disabled = page.locator(".task-composer button:disabled");
    await page.mouse.move(0, 0);
    const disabledIdle = await disabled.evaluate(paint);
    await disabled.hover();
    await page.mouse.down();
    expect(await disabled.evaluate(paint)).toEqual(disabledIdle);
    await page.mouse.up();
    expect(await disabled.isEnabled()).toBe(false);
    const labels = page.locator("#task-labels");
    await labels.fill("ui, selection");
    await labels.press("Home");
    await labels.press("Shift+End");
    expect(await labels.evaluate((node) => node.value.slice(node.selectionStart, node.selectionEnd))).toBe("ui, selection");
    await page.locator("#task-comment").fill("Keep comments editable.");
    expect(await page.locator("#task-comment").inputValue()).toBe("Keep comments editable.");
    await mountInteractionFixture(page, basePath, { extras: true });
    await page.locator("#interaction-readonly").click();
    await page.locator("#interaction-readonly").press("ControlOrMeta+A");
    expect(await page.locator("#interaction-readonly").evaluate((node) => node.value.slice(node.selectionStart, node.selectionEnd))).toBe("Saved diagnostic text");
    await page.locator("#interaction-editor").fill("Editable content stays editable.");
    expect(await page.locator("#interaction-editor").textContent()).toBe("Editable content stays editable.");
  });
}, 60_000);

it("paints pressed feedback over existing archive and commit row shadows", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountInteractionFixture(page, basePath, { extras: true });
    for (const selector of [".card.archive-row", ".crow.ahead", ".crow.unpushed", ".crow.sel"]) {
      const row = page.locator(selector);
      await row.hover();
      const paint = (node) => ({ shadow: getComputedStyle(node).boxShadow, filter: getComputedStyle(node).filter });
      const hovered = await row.evaluate(paint);
      await page.mouse.down();
      expect(await row.evaluate((node) => node.matches(":active")), selector).toBe(true);
      expect(await row.evaluate(paint), selector).not.toEqual(hovered);
      await page.mouse.up();
    }
  });
}, 60_000);

it("keeps inline prose links free of block tints and puts row navigation on its anchor", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountInteractionFixture(page, basePath, { extras: true });
    const link = page.locator('.task-comment-body a[href="https://docs.example.test/interaction"]');
    await link.hover();
    expect(await link.evaluate((node) => getComputedStyle(node).boxShadow)).toBe("none");
    expect(await link.evaluate((node) => getComputedStyle(node).webkitTapHighlightColor)).not.toBe("rgba(0, 0, 0, 0)");
    await page.mouse.down();
    expect(await link.evaluate((node) => node.matches(":active"))).toBe(true);
    expect(await link.evaluate((node) => getComputedStyle(node).boxShadow)).toBe("none");
    await page.mouse.move(0, 0);
    await page.mouse.up();
    const row = page.locator(".task-row");
    await row.scrollIntoViewIfNeeded();
    const target = await row.evaluate((node) => {
      const box = node.getBoundingClientRect();
      const hit = document.elementFromPoint(box.right - 3, box.top + 3);
      return { anchor: Boolean(hit.closest(".task-row-open[href]")), cursor: getComputedStyle(hit).cursor };
    });
    expect(target).toEqual({ anchor: true, cursor: "pointer" });
  });
}, 60_000);

it("keeps a real touch tap on a legacy tab actionable without selecting its label", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountInteractionFixture(page, basePath);
    const tab = page.locator('.tabs .t[data-tab="changes"]');
    const box = await tab.boundingBox();
    const session = await page.context().newCDPSession(page);
    try {
      await session.send("Emulation.setTouchEmulationEnabled", { enabled: true });
      await session.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x: box.x + box.width / 2, y: box.y + box.height / 2 }] });
      await session.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
      await page.waitForFunction(() => window.__interactionTab === "changes");
      expect(await page.evaluate(() => window.getSelection().toString())).toBe("");
      expect(await tab.evaluate((node) => getComputedStyle(node).userSelect)).toBe("none");
    } finally { await session.detach(); }
  }, { width: 390, height: 844 });
}, 60_000);

it("preserves real Ghostty mouse selection and terminal touch scrolling", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountInteractionFixture(page, basePath, { extras: true });
    await loadBrowserModules(page, { pane: "src/terminal/pane.js" }, basePath);
    const host = page.locator("#interaction-terminal");
    await host.scrollIntoViewIfNeeded();
    await page.evaluate(async () => {
      window.__interactionPane = await window.__layoutModules.pane.mountTerminalPane(document.querySelector("#interaction-terminal"), {
        attach: async () => {}, input: async () => {}, resize: async () => {}, isTouchDevice: () => true,
      });
      await new Promise((resolve) => window.__buildTerminal.write("SELECTION_PROBE terminal text\r\n", resolve));
    });
    try {
      const grid = await host.evaluate((node) => {
        const box = node.querySelector("canvas").getBoundingClientRect();
        const metrics = window.__buildTerminal.renderer.getMetrics();
        return { x: box.x, y: box.y, width: metrics.width, height: metrics.height };
      });
      await page.mouse.move(grid.x + grid.width * 0.3, grid.y + grid.height * 0.5);
      await page.mouse.down();
      await page.mouse.move(grid.x + grid.width * 14.4, grid.y + grid.height * 0.5, { steps: 12 });
      await page.mouse.up();
      expect(await page.evaluate(() => window.__buildTerminal.getSelection())).toBe("SELECTION_PROBE");
      expect(await page.evaluate(() => window.__buildTerminal.hasSelection())).toBe(true);
      expect(await host.locator(".term-screen").evaluate((node) => getComputedStyle(node).touchAction)).toBe("none");
      expect(await host.locator(".termkey").first().evaluate((node) => getComputedStyle(node).touchAction)).toBe("pan-x");
    } finally { await page.evaluate(() => window.__interactionPane.dispose()); }
  }, { width: 1280, height: 900 });
}, 60_000);
