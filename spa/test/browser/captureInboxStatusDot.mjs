// Review captures for #380, using the same production fixture as its layout gate.
// Run from spa/: CAPTURE_PHASE=before|after nice -n 10 node test/browser/captureInboxStatusDot.mjs
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { inboxTextPositions, mountStatusDotInbox } from "./inboxStatusDotSeed.mjs";

const phase = process.env.CAPTURE_PHASE || "after";
const directory = process.env.CAPTURE_DIR || "/tmp/inbox-status-dot-evidence";
await mkdir(directory, { recursive: true });
const positions = {};
for (const { label, width, height } of [
  { label: "desktop", width: 1280, height: 800 },
  { label: "mobile", width: 390, height: 760 },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    for (const { scene, grouped, folded } of [
      { scene: "ungrouped", grouped: false, folded: false },
      { scene: "grouped-expanded", grouped: true, folded: false },
      { scene: "grouped-folded", grouped: true, folded: true },
    ]) {
      await mountStatusDotInbox(page, basePath, { grouped, folded });
      positions[`${label}-${scene}`] = await inboxTextPositions(page);
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
