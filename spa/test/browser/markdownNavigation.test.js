import { expect, it } from "vitest";
import { markdownHtml } from "../../src/core/markdown.js";
import { mountLayout, withLayoutPage } from "./layoutHarness.mjs";

it("keeps keyboard footnote navigation in the current route and native details operable", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    const html = markdownHtml("Text[^a]\n\n<details>\n<summary>More details</summary>\nHidden **content**\n</details>\n\n[^a]: Accessible note", { taskLabelScope: "browser-footnotes" });
    await mountLayout(page, `<article class="markdown">${html}</article>`, { basePath });
    await page.addScriptTag({ type: "module", content: `
      import { installMarkdownNavigation } from "${basePath}src/core/markdownNavigation.js";
      installMarkdownNavigation();
      history.replaceState(null, "", "#/projects/example/tasks/1");
      window.markdownNavigationReady = true;
    ` });
    await page.waitForFunction(() => window.markdownNavigationReady);
    const original = page.url();
    await page.locator(".md-footnote-ref a").focus();
    await page.keyboard.press("Enter");
    expect(page.url()).toBe(original);
    expect(await page.evaluate(() => document.activeElement?.matches(".md-footnotes > ol > li"))).toBe(true);
    await page.locator(".md-footnote-backref").focus();
    await page.keyboard.press("Enter");
    expect(page.url()).toBe(original);
    expect(await page.evaluate(() => document.activeElement?.matches(".md-footnote-ref a"))).toBe(true);
    await page.locator("summary").focus();
    await page.keyboard.press("Space");
    await expect.poll(() => page.locator("details").getAttribute("open")).toBe("");
    expect(await page.locator("details strong").isVisible()).toBe(true);
  });
});
