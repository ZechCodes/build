// Isolated Projects face evidence (#480, #510, #516). Run from spa/ with CAPTURE_PHASE=before|after.
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { inspectProjectsFace, mountProjectsFace, openRecent, settleTheme, hoverRecentRow } from "./projectsFaceSeed.mjs";

const phase = process.env.CAPTURE_PHASE || "after";
const directory = process.env.CAPTURE_DIR || "/tmp/projects-face-480";
await mkdir(directory, { recursive: true });
const evidence = {};
for (const { label, width, height, hasTouch } of [
  { label: "desktop", width: 1280, height: 800, hasTouch: false },
  { label: "phone", width: 390, height: 844, hasTouch: true },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountProjectsFace(page, basePath, { standing: true });
    for (const theme of ["light", "dark"]) {
      await settleTheme(page, theme);
      const blocks = await inspectProjectsFace(page);
      evidence[`${label}-${theme}`] = blocks;
      if (phase === "after") {
        assert.deepEqual(blocks.map((block) => block.name), ["Zulu", "Build", "Alpha", "Bravo"]);
        for (const block of blocks) {
          assert.equal(block.background, block.active ? block.accentSoft : "rgba(0, 0, 0, 0)");
          assert.equal(block.border, "0px");
          assert.equal(block.shadow, "none");
        }
        assert.equal(await page.locator("#inbox-list > .inbox-recent").count(), 0);
        assert.equal(await page.locator('.inbox-fold[aria-expanded="true"]').count(), 4);
        assert.equal(await page.locator('.inbox-project .inbox-recent-toggle[aria-expanded="false"]').count(), 2);
        assert.equal(await page.locator('[data-recent-toggle="projects-review/alpha"] .inbox-recent-count').textContent(), "1");
        assert.equal(await page.locator('.inbox-project[data-project="projects-review/alpha"] .inbox-entry').count(), 1);
      }
      await page.locator("#inbox-rail").screenshot({ path: join(directory, `${phase}-${label}-${theme}.png`) });
    }
    // Matched #540 evidence: Recent open, a standing row and a hovered sibling.
    await openRecent(page, "projects-review/build");
    for (const theme of ["light", "dark"]) {
      await settleTheme(page, theme);
      await hoverRecentRow(page);
      assert.equal(await page.locator(".inbox-project.active .inbox-recent .inbox-entry.active").count(), 1);
      assert.equal(await page.locator(".inbox-project.active .inbox-entry:hover").count(), 1);
      await page.locator("#inbox-rail").screenshot({ path: join(directory, `${phase}-${label}-${theme}-recent.png`) });
    }
  }, { width, height, hasTouch });
}
await writeFile(join(directory, `${phase}-styles.json`), `${JSON.stringify(evidence, null, 2)}\n`);
