import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, it } from "vitest";
import { captureLayout, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// The top of the app on an iPad (#193). Since Safari 26 WebKit takes the colour
// it runs into the status bar from the top edge: it hit-tests a few px in at
// the edge's centre, climbs to the first fixed or sticky box, and reads that
// box's background-color if the box spans the viewport and stands taller than
// 10px (LocalFrameView::fixedContainerEdges). With no such box iOS fills the
// inset with its own blur, which ramps down over the toolbar. This pins what
// WebKit looks for, against the real shell markup and production sheets.

const shellMarkup = readFileSync(resolve("index.html"), "utf8")
  .match(/<body>([\s\S]*)<\/body>/)[1]
  .replace('<div id="toolbar"></div>', `<div id="toolbar"><div class="toolbar">
    <button class="tb-sel tb-project" type="button"><span class="tb-name">Build</span></button>
    <button class="tb-sel" type="button">Tasks</button></div></div>`);

// Landscape with the inbox pinned or away; portrait, where it is away at rest
// (open, it is an overlay whose scrim is the top edge, as it should be).
const IPAD = [
  { width: 1180, height: 820, inbox: "pinned" },
  { width: 1180, height: 820, inbox: "away" },
  { width: 820, height: 1180, inbox: "away" },
];

/** What WebKit would find at the top edge, and who owns the regions' top rows. */
const topEdge = (page) => page.evaluate(() => {
  const fixedContainerAt = (x, y) => {
    let element = document.elementFromPoint(x, y);
    while (element && !["fixed", "sticky"].includes(getComputedStyle(element).position)) element = element.parentElement;
    return element;
  };
  const container = fixedContainerAt(innerWidth / 2, 4);
  const rect = container?.getBoundingClientRect();
  const topRowOwnedBy = (selector) => {
    const host = document.querySelector(selector);
    const box = host.getBoundingClientRect();
    if (!box.width) return null;
    return [box.left + 12, (box.left + box.right) / 2, box.right - 12]
      .every((x) => host.contains(document.elementFromPoint(x, box.top + 0.5)));
  };
  return {
    container: container?.id || null,
    width: rect?.width, height: rect?.height, viewport: innerWidth,
    background: container && getComputedStyle(container).backgroundColor,
    page: getComputedStyle(document.body).backgroundColor,
    toolbarTopRow: topRowOwnedBy("#toolbar"),
    // Away, the inbox is a card that opens 48px down, nowhere near the band.
    inboxTopRow: document.body.classList.contains("inbox-collapsed") ? null : topRowOwnedBy("#inbox-rail"),
  };
});

for (const theme of ["dark", "light"]) {
  for (const { inbox, ...viewport } of IPAD) {
    const name = `${theme}-${viewport.width}x${viewport.height}-inbox-${inbox}`;
    it(`gives WebKit the page colour at the top edge, ${name}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await mountLayout(page, shellMarkup, { basePath });
        await page.evaluate(({ theme, away }) => {
          document.documentElement.dataset.theme = theme;
          document.body.classList.toggle("inbox-collapsed", away);
        }, { theme, away: inbox === "away" });
        const edge = await topEdge(page);
        expect(edge.container).toBe("status-edge");
        expect(edge.width).toBe(edge.viewport);
        expect(edge.height).toBeGreaterThan(10);
        // A plain, opaque colour: the band of page the regions float on.
        expect(edge.background).toBe(edge.page);
        expect(edge.background).toMatch(/^rgb\(/);
        // The box stays in that band: the regions paint over the rows it could reach.
        expect(edge.toolbarTopRow).toBe(true);
        expect(edge.inboxTopRow).not.toBe(false);
        await captureLayout(page, `status-edge-${name}.png`);
      }, viewport);
    });
  }
}

// While the new-version banner stands it is the top edge, so it must be the box
// WebKit finds there: in flow alone it offered nothing, and iOS blurred the
// inset down over the banner (#193 follow-up).
for (const theme of ["dark", "light"]) {
  it(`gives WebKit the version banner's colour at the top edge while it stands, ${theme}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, shellMarkup, { basePath });
      const before = await page.evaluate((theme) => {
        document.documentElement.dataset.theme = theme;
        document.body.classList.add("inbox-collapsed");
        const banner = document.getElementById("verbar");
        banner.hidden = false;
        const { top, height } = banner.getBoundingClientRect();
        return { top, height };
      }, theme);
      const edge = await topEdge(page);
      expect(edge.container).toBe("verbar");
      expect(edge.width).toBe(edge.viewport);
      expect(edge.height).toBeGreaterThan(10);
      // A plain, opaque colour: the banner's own.
      expect(edge.background).toMatch(/^rgb\(/);
      expect(edge.background).not.toBe(edge.page);
      // The banner keeps its place, and the toolbar still owns its top row.
      expect(before.top).toBe(0);
      expect(edge.toolbarTopRow).toBe(true);
      expect(await page.evaluate(() => document.getElementById("status-edge").getClientRects().length)).toBe(0);
      await captureLayout(page, `status-edge-verbar-${theme}-1180x820.png`);
    }, { width: 1180, height: 820 });
  });
}
