import { expect, it } from "vitest";
import { markdownHtml } from "../../src/core/markdown.js";
import { mountLayout, withLayoutPage } from "./layoutHarness.mjs";

for (const width of [320, 960]) {
  it(`keeps footnote return links on the final text line at ${width}px`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      const source = "A note[^a], cited again[^a].\n\n[^a]: First paragraph.\n\n    The final paragraph has enough text to wrap on a narrow display. Last words.";
      await mountLayout(page, `<article class="markdown">${markdownHtml(source)}</article>`, {
        basePath, styles: "body { padding:12px; } article { max-width:650px; }",
      });
      const geometry = await page.locator(".md-footnotes li").evaluate((note) => {
        const paragraphs = note.querySelectorAll("p");
        const last = paragraphs[paragraphs.length - 1];
        const text = last.firstChild;
        const range = document.createRange();
        const end = text.textContent.trimEnd().length;
        range.setStart(text, end - 1);
        range.setEnd(text, end);
        const line = range.getBoundingClientRect();
        return [...note.querySelectorAll(".md-footnote-backref")].map((link) => ({
          inParagraph: link.parentElement === last,
          top: link.getBoundingClientRect().top, textTop: line.top,
        }));
      });
      expect(geometry).toHaveLength(2);
      for (const link of geometry) {
        expect(link.inParagraph).toBe(true);
        expect(link.top).toBe(link.textTop);
      }
    }, { width, height: 600 });
  });
}
