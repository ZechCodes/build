// Isolated before/after evidence for #389. Run from spa/ with CAPTURE_PHASE=before|after.
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountStatusDotInbox } from "./inboxStatusDotSeed.mjs";

const phase = process.env.CAPTURE_PHASE || "after";
const directory = process.env.CAPTURE_DIR || "/tmp/inbox-project-hover-evidence";
await mkdir(directory, { recursive: true });
const positions = {};
for (const { label, width, height } of [
  { label: "desktop", width: 1280, height: 800 },
  { label: "mobile", width: 390, height: 760 },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    for (const theme of ["light", "dark"]) {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountStatusDotInbox(page, basePath, { grouped: true });
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      const head = page.locator(".inbox-project-head");
      await head.locator(".inbox-project-name").hover();
      await page.waitForFunction(() => getComputedStyle(document.querySelector(".inbox-project-actions")).opacity === "1");
      positions[`${label}-${theme}`] = await head.evaluate((element) => Object.fromEntries(
        [...element.querySelectorAll("button, .inbox-status-dot")].map((node) => {
          const { x, y, width, height } = node.getBoundingClientRect();
          return [node.className, { x, y, width, height }];
        }),
      ));
      await page.locator("#inbox-rail").screenshot({ path: join(directory, `${phase}-${label}-${theme}-head-hover.png`) });
    }
  }, { width, height });
}
await writeFile(join(directory, `${phase}-head-positions.json`), `${JSON.stringify(positions, null, 2)}\n`);
