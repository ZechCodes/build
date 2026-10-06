// Review captures for #381. Run from spa/ with CAPTURE_PHASE=before|after.
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountStatusDotInbox } from "./inboxStatusDotSeed.mjs";

const phase = process.env.CAPTURE_PHASE || "after";
const directory = process.env.CAPTURE_DIR || "/tmp/inbox-overlay-fill-evidence";
await mkdir(directory, { recursive: true });
await withLayoutPage(async ({ page, basePath }) => {
  for (const theme of ["light", "dark"]) {
    await mountStatusDotInbox(page, basePath, { grouped: true });
    await page.evaluate((theme) => {
      document.documentElement.dataset.theme = theme;
      document.querySelector(".inbox-project").classList.add("active");
    }, theme);
    for (const { label, key } of [
      { label: "task", key: "task:layout-device/layout-project/380" },
      { label: "quiet", key: "branch:quiet" },
    ]) {
      const row = page.locator(`.inbox-entry[data-key="${key}"]`);
      await row.hover();
      await page.waitForFunction((key) =>
        getComputedStyle(document.querySelector(`[data-key="${key}"] .inbox-more`)).opacity === "1", key);
      await page.locator("#inbox-rail").screenshot({ path: join(directory, `${phase}-${theme}-${label}-hover.png`) });
      await row.screenshot({ path: join(directory, `${phase}-${theme}-${label}-row.png`) });
    }
  }
});
