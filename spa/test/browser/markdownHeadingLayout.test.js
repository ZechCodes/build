import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountPlanMarkdown } from "./markdownSurfacesFixture.mjs";

for (const theme of ["light", "dark"]) {
  it(`distinguishes level three from the lower heading levels in a real ${theme} plan preview`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountPlanMarkdown(page, basePath, theme);
      const headings = await page.locator(".plan :is(h3, h4, h5, h6)").evaluateAll((nodes) =>
        nodes.map((node) => ({
          size: Number.parseFloat(getComputedStyle(node).fontSize),
          weight: Number.parseFloat(getComputedStyle(node).fontWeight),
          color: getComputedStyle(node).color,
          text: node.textContent,
        })));
      expect(headings.map((heading) => heading.text)).toEqual(["Verification", "Keyboard access", "Privacy", "Evidence"]);
      expect(headings[0].size - headings[1].size).toBeGreaterThanOrEqual(2);
      expect(headings[1].size - headings[2].size).toBeGreaterThanOrEqual(1);
      expect(headings[2].size - headings[3].size).toBeGreaterThanOrEqual(1);
      expect(headings[0].weight).toBeGreaterThanOrEqual(headings[1].weight);
      expect(headings[1].weight).toBeGreaterThan(headings[2].weight);
      expect(new Set(headings.map((heading) => heading.color)).size).toBe(1);
    }, { width: 1180, height: 1000 });
  });
}
