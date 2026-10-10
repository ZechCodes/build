// Isolated #480 evidence. Run from spa/ with CAPTURE_PHASE=before|after.
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { inspectProjectsFace, mountProjectsFace } from "./projectsFaceSeed.mjs";

const phase = process.env.CAPTURE_PHASE || "after";
const directory = process.env.CAPTURE_DIR || "/tmp/projects-face-480";
await mkdir(directory, { recursive: true });
const evidence = {};
for (const { label, width, height, hasTouch } of [
  { label: "desktop", width: 1280, height: 800, hasTouch: false },
  { label: "phone", width: 390, height: 844, hasTouch: true },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountProjectsFace(page, basePath);
    for (const theme of ["light", "dark"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      const blocks = await inspectProjectsFace(page);
      evidence[`${label}-${theme}`] = blocks;
      if (phase === "after") {
        assert.deepEqual(blocks.map((block) => block.name), ["Alpha", "Bravo", "Build", "Zulu"]);
        for (const block of blocks) {
          assert.equal(block.background, "rgba(0, 0, 0, 0)");
          assert.equal(block.border, "0px");
          assert.equal(block.shadow, "none");
        }
        assert.equal(await page.locator("#inbox-list > .inbox-recent").count(), 0);
      }
      await page.locator("#inbox-rail").screenshot({ path: join(directory, `${phase}-${label}-${theme}.png`) });
    }
  }, { width, height, hasTouch });
}
await writeFile(join(directory, `${phase}-styles.json`), `${JSON.stringify(evidence, null, 2)}\n`);
