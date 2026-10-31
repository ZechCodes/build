import { expect, it } from "vitest";
import { captureLayout, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// The two overlays a terminal pane can carry at once: the reconnecting chip
// (top centre) and the saved-transport label. The agent pane attaches the chip
// first; the console attaches the label first. Both orders are pinned.
const CHIP = '<div class="term-conn">reconnecting to your machine…</div>';
const LABEL = '<div class="term-transport">Terminal over WebRTC</div>';
const ORDERS = { "chip first": CHIP + LABEL, "label first": LABEL + CHIP };
const WIDTHS = [320, 390, 1280];
const THEMES = ["light", "dark"];

async function mountPane(page, basePath, { width, theme, inner, offline }) {
  await page.setViewportSize({ width, height: 400 });
  await mountLayout(page, `<div class="termpane${offline ? " term-offline" : ""}" id="pane">${inner}</div>`, {
    basePath, styles: "body { margin:0; } #pane { width:100vw; height:300px; }",
  });
  await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
  return page.evaluate(() => {
    const box = (selector) => {
      const node = document.querySelector(selector);
      if (!node || node.hidden) return null;
      const { left, right, top, bottom } = node.getBoundingClientRect();
      return { left, right, top, bottom };
    };
    return { pane: box("#pane"), chip: box(".term-conn"), label: box(".term-transport") };
  });
}

const overlaps = (a, b) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
const inside = (inner, outer) => inner.left >= outer.left && inner.right <= outer.right
  && inner.top >= outer.top && inner.bottom <= outer.bottom;

it("keeps the transport label clear of the reconnecting chip at every pane width, in both themes", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    for (const width of WIDTHS) {
      for (const theme of THEMES) {
        for (const [order, inner] of Object.entries(ORDERS)) {
          const where = `${width}px ${theme} ${order}`;
          const { pane, chip, label } = await mountPane(page, basePath, { width, theme, inner, offline: true });
          expect(chip, where).not.toBeNull();
          expect(overlaps(chip, label), `${where}: ${JSON.stringify({ chip, label })}`).toBe(false);
          expect(inside(label, pane), where).toBe(true);
          expect(pane.right - label.right, where).toBeCloseTo(12, 0);
          await captureLayout(page, `terminal-transport-${width}-${theme}-${order.replace(" ", "-")}.png`);
        }
      }
    }
  });
}, 60_000);

it("puts the label in the same bottom corner whether or not the chip shows", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    for (const width of WIDTHS) {
      const hiddenChip = '<div class="term-conn" hidden>reconnecting to your machine…</div>';
      const quiet = await mountPane(page, basePath, { width, theme: "light", inner: hiddenChip + LABEL, offline: false });
      const offline = await mountPane(page, basePath, { width, theme: "light", inner: CHIP + LABEL, offline: true });
      expect(quiet.chip, `${width}px`).toBeNull();
      expect(quiet.pane.bottom - quiet.label.bottom, `${width}px`).toBeCloseTo(8, 0);
      expect(quiet.pane.right - quiet.label.right, `${width}px`).toBeCloseTo(12, 0);
      expect(offline.label, `${width}px`).toEqual(quiet.label);
    }
  });
}, 60_000);
