// Review captures and geometry for #646, using the production layout fixture.
// Run from spa/: CAPTURE_PHASE=before|after nice -n 10 node test/browser/captureInboxStatusDot.mjs
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountStatusDotInbox } from "./inboxStatusDotSeed.mjs";

const phase = process.env.CAPTURE_PHASE || "after";
const directory = process.env.CAPTURE_DIR || "/tmp/inbox-status-dot-evidence";
await mkdir(directory, { recursive: true });
const positions = {};
for (const { label, width, height, floating = false } of [
  { label: "desktop", width: 1280, height: 800 },
  { label: "desktop-floating", width: 1280, height: 800, floating: true },
  { label: "mobile", width: 390, height: 760 },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    for (const { scene, grouped, folded } of [
      { scene: "ungrouped", grouped: false, folded: false },
      { scene: "grouped-expanded", grouped: true, folded: false },
      { scene: "grouped-folded", grouped: true, folded: true },
    ]) {
      await mountStatusDotInbox(page, basePath, { grouped, folded });
      if (floating) await page.evaluate(() => document.body.classList.add("inbox-collapsed", "inbox-popover-open"));
      positions[`${label}-${scene}`] = await page.locator(".inbox-entry:visible, .inbox-project-head:visible").evaluateAll((rows) =>
        Object.fromEntries(rows.map((row) => {
          const box = (element) => {
            if (!element) return null;
            const { x, y, width, height } = element.getBoundingClientRect();
            return { x, y, width, height };
          };
          return [row.dataset.key || "project-head", { name: box(row.querySelector(".stitle, .inbox-project-name")),
            dot: box(row.querySelector(".inbox-status-dot")), fold: box(row.querySelector(".inbox-fold")) }];
        })));
      const hovered = grouped ? page.locator(".inbox-project-head") : page.locator(".inbox-entry").first();
      await hovered.hover();
      if (grouped) await page.waitForFunction(() => getComputedStyle(document.querySelector(".inbox-project-actions")).opacity === "1");
      await page.locator("#inbox-rail").screenshot({ path: join(directory, `${phase}-${label}-${scene}-hover.png`) });
      if (grouped) await hovered.screenshot({ path: join(directory, `${phase}-${label}-${scene}-head-hover.png`) });
      for (const row of await page.locator(".inbox-entry:visible").all()) {
        await row.hover();
        const rowKey = await row.getAttribute("data-key");
        await page.waitForFunction((key) => [...document.querySelector(`[data-key="${key}"] .inbox-actions`).children]
          .filter((child) => child.matches("button")).every((button) => getComputedStyle(button).opacity === "1"), rowKey);
        const key = rowKey.replaceAll(/[^a-z0-9-]/gi, "-");
        await row.screenshot({ path: join(directory, `${phase}-${label}-${scene}-${key}-hover.png`) });
      }
    }
  }, { width, height });
}
await writeFile(join(directory, `${phase}-text-positions.json`), `${JSON.stringify(positions, null, 2)}\n`);
