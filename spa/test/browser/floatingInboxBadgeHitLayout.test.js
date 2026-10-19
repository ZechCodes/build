import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

const settle = (page) => page.evaluate(() => new Promise((resolve) => {
  requestAnimationFrame(() => requestAnimationFrame(resolve));
}));

const switcherHits = (page) => page.evaluate(() => {
  const switcher = document.querySelector(".tb-switch");
  const target = switcher.getBoundingClientRect();
  const view = document.querySelector("#view").getBoundingClientRect();
  const hits = [];
  for (let y = 0.5; y < target.height; y++) for (let x = 0.5; x < target.width; x++) {
    const hit = document.elementFromPoint(target.left + x, target.top + y);
    hits.push({ x, y, owned: switcher.contains(hit) });
  }
  return { hits, width: target.width, height: target.height,
    glyphX: switcher.querySelector("svg").getBoundingClientRect().left - view.left };
});

for (const width of [390, 901, 1280]) for (const inset of [0, 20]) for (const directory of [true, false]) {
  it(`keeps the unread badge off the switcher hit area at ${width}px, safe-area ${inset}px, rail ${directory}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      const cdp = await page.context().newCDPSession(page);
      await cdp.send("Emulation.setSafeAreaInsetsOverride", { insets: { left: inset } });
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountNavigationBreadcrumbs(page, basePath, { kind: "project", inboxHeader: true });
      await page.evaluate(async (directory) => {
        if (!directory) document.querySelector("#dir-rail").replaceChildren();
        await window.__layoutModules.inbox.setInboxCollapsed(true, { animate: false, persist: false, reveal: false });
        document.querySelector("#inbox-open").classList.remove("has-attention");
      }, directory);
      await settle(page);
      const before = await switcherHits(page);
      expect(before.width).toBe(44);
      expect(before.height).toBe(44);
      expect(before.glyphX).toBe(60 + inset);
      expect(before.hits).toHaveLength(44 * 44);
      expect(before.hits.some((hit) => hit.owned)).toBe(true);

      await page.evaluate(() => {
        document.querySelector("#inbox-open").classList.add("has-attention");
        document.querySelector(".inbox-open-count").textContent = "99+";
      });
      await settle(page);
      const after = await switcherHits(page);
      expect(after.width).toBe(44);
      expect(after.height).toBe(44);
      expect(after.glyphX).toBe(before.glyphX);
      // Rounded corners already exclude a few pixel centres. The badge must
      // leave every point in the existing switcher hit shape reachable.
      const blocked = after.hits.filter((hit, index) => before.hits[index].owned && !hit.owned);
      expect(blocked.length, JSON.stringify(blocked.slice(0, 3))).toBe(0);

      const inboxPoints = await page.locator("#inbox-open").evaluate((button) => {
        const box = button.getBoundingClientRect();
        const badge = button.querySelector(".inbox-open-count").getBoundingClientRect();
        return [
          { x: box.left + box.width / 2, y: box.top + box.height / 2 },
          { x: (Math.max(box.left, badge.left) + Math.min(box.right, badge.right)) / 2,
            y: (Math.max(box.top, badge.top) + Math.min(box.bottom, badge.bottom)) / 2 },
        ].map((point) => ({ ...point, owned: button.contains(document.elementFromPoint(point.x, point.y)) }));
      });
      for (const point of inboxPoints) {
        expect(point.owned).toBe(true);
        await page.mouse.click(point.x, point.y);
        expect(await page.locator("body").evaluate((body) => body.classList.contains("inbox-popover-open"))).toBe(true);
        await page.keyboard.press("Escape");
        expect(await page.locator("body").evaluate((body) => body.classList.contains("inbox-popover-open"))).toBe(false);
      }
    }, { width, height: 800 });
  });
}
