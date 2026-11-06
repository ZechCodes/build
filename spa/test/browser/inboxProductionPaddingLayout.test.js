import { build } from "vite";
import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { mountStatusDotInbox } from "./inboxStatusDotSeed.mjs";

let session;
let productionCss;

beforeAll(async () => {
  // Build the real entry: source fixtures inject shell.css again after the
  // base styles, hiding padding regressions caused by production chunk order.
  const bundle = await build({ logLevel: "silent", build: { write: false } });
  const assets = new Map(bundle.output.filter((asset) => asset.type === "asset")
    .map((asset) => [asset.fileName, asset.source]));
  productionCss = [...String(assets.get("index.html")).matchAll(/<link[^>]+href="\/app\/static\/([^\"]+\.css)"/g)]
    .map(([, name]) => assets.get(name)).join("\n");
  expect(productionCss).toContain(".inbox-entry");
  session = await openLayoutSession();
});
afterAll(async () => { await session?.close(); });

for (const { label, width, floating } of [
  { label: "pinned desktop", width: 1280, floating: false },
  { label: "floating desktop", width: 1280, floating: true },
  { label: "floating phone", width: 390, floating: true },
]) {
  it.each([false, true])(`preserves main's row padding and dots in the production ${label} (grouped: %s)`, async (grouped) => {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountStatusDotInbox(page, basePath, { grouped });
      await page.evaluate(({ css, floating }) => {
        document.querySelectorAll('link[rel="stylesheet"], style').forEach((node) => node.remove());
        const sheet = document.createElement("style");
        sheet.textContent = css;
        document.head.appendChild(sheet);
        document.body.classList.toggle("inbox-collapsed", floating);
        document.body.classList.toggle("inbox-popover-open", floating);
        document.querySelector('[data-key="workspace:layout-device/attention"]').classList.add("active");
      }, { css: productionCss, floating });
      const measurements = await page.locator(".inbox-entry").evaluateAll((rows) => rows.map((row) => {
        const rail = document.querySelector("#inbox-rail");
        const panel = rail.getBoundingClientRect();
        const bounds = row.getBoundingClientRect();
        const name = row.querySelector(".stitle").getBoundingClientRect();
        const dot = row.querySelector(".inbox-status-dot")?.getBoundingClientRect();
        const css = getComputedStyle(row);
        const innerLeft = panel.left + rail.clientLeft;
        return { key: row.dataset.key, quiet: row.classList.contains("inbox-quiet"),
          padding: [css.paddingTop, css.paddingRight, css.paddingBottom, css.paddingLeft],
          left: bounds.left - innerLeft, right: panel.left + rail.clientLeft + rail.clientWidth - bounds.right,
          name: name.left - innerLeft, dot: dot && { left: dot.left - innerLeft, right: dot.right - innerLeft },
          radius: css.borderRadius };
      }));
      console.log(`Production inbox geometry ${label}, grouped=${grouped}: ${JSON.stringify(measurements)}`);
      for (const row of measurements) {
        // Main's fill spans the panel; its text and entire status dot are inset.
        expect.soft([row.left, row.right], row.key).toEqual([0, 0]);
        expect.soft(row.radius, row.key).toBe("0px");
        expect.soft(row.name, row.key).toBe(grouped ? 41 : row.quiet ? 36 : 23);
        expect.soft(row.padding[0], row.key).toBe(row.quiet ? "8px" : "10px");
        expect.soft(row.padding[2], row.key).toBe(row.quiet ? "8px" : "10px");
        expect.soft(row.padding[1], row.key).toBe(row.quiet ? grouped ? "55px" : "50px" : grouped ? "28px" : "23px");
        if (row.dot) {
          expect.soft(row.dot.left, row.key).toBeGreaterThanOrEqual(10);
          expect.soft(row.dot.right, row.key).toBeLessThan(row.name);
        }
      }
    }, { width, height: 800, hasTouch: width === 390 });
  });
}
