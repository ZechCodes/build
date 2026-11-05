import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountFilesExplorer, openFile } from "./filesExplorerSeed.mjs";
import { dragInteractionText } from "./interactionFixture.mjs";

it("allows native selection and copy in rendered Markdown and source without selecting gutters", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountFilesExplorer(page, basePath);
    await openFile(page, "CLAUDE.md");
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    for (const selector of [".file-reading-layer .plan h1", ".file-reading-layer .fsrc-code"]) {
      const target = page.locator(selector).first();
      await target.waitFor();
      expect(await target.evaluate((node) => getComputedStyle(node).userSelect)).toBe("text");
      expect(await target.evaluate((node) => getComputedStyle(node).webkitTouchCallout)).not.toBe("none");
      const selected = await dragInteractionText(page, selector);
      expect(selected.length).toBeGreaterThan(0);
      await page.keyboard.press("ControlOrMeta+C");
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(selected);
      if (selector.includes(".plan")) await page.locator('[data-file-mode="source"]').click();
    }
    expect(await page.locator(".fsrc-ln").first().evaluate((node) => getComputedStyle(node).userSelect)).toBe("none");
    expect(await dragInteractionText(page, ".fsrc-ln")).toBe("");
  });
}, 60_000);
