import { expect, it } from "vitest";
import { markdownHtml } from "../../src/core/markdown.js";
import { captureLayout, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// #253: a markdown thematic break is the app's own divider, on every surface
// that reads markdown: one hairline in --line2, a 12px margin that collapses
// with the paragraphs' around it, and never the browser's inset two-tone line.
// Pinned in a real Chromium, because the browser's default hr (inset borders,
// auto margins, its own height) is exactly what jsdom cannot see.

const RULED = "The first section ends here.\n\n---\n\nThe second section starts here.";

/** The two surfaces a comment or a message is read on, in their real markup. */
const HOSTS = {
  "task comment": (html) => `<ol class="task-timeline"><li class="task-entry task-comment">
    <div class="task-comment-card"><div class="task-comment-body markdown">${html}</div></div></li></ol>`,
  "chat message": (html) => `<div class="thread-message"><div class="thread-comment-card"><div class="thread-body markdown">${html}</div></div></div>`,
};

/** The gap either side of the rule on each surface, with margins collapsed. */
const GAP = { "task comment": 14, "chat message": 12 };

/** The rule's borders and box, the theme's --line2 resolved to a colour, and
 *  the gaps between the rule and the paragraphs either side of it. */
const measureRule = (page) => page.evaluate(() => {
  const rule = document.querySelector("hr");
  const style = getComputedStyle(rule);
  const probe = document.createElement("span");
  probe.style.color = "var(--line2)";
  rule.parentElement.append(probe);
  const line2 = getComputedStyle(probe).color;
  probe.remove();
  const box = rule.getBoundingClientRect();
  const above = rule.previousElementSibling.getBoundingClientRect();
  const below = rule.nextElementSibling.getBoundingClientRect();
  const px = (element, property) => parseFloat(getComputedStyle(element).getPropertyValue(property));
  const side = (name) => ({
    width: style.getPropertyValue(`border-${name}-width`),
    style: style.getPropertyValue(`border-${name}-style`),
    color: style.getPropertyValue(`border-${name}-color`),
  });
  return {
    className: rule.className,
    line2,
    top: side("top"),
    others: ["right", "bottom", "left"].map((name) => side(name)),
    // The content box: under the app's border-box sizing, computed height counts the border.
    contentHeight: rule.clientHeight,
    boxHeight: box.height,
    margins: [style.marginTop, style.marginBottom],
    gapAbove: box.top - above.bottom,
    gapBelow: below.top - box.bottom,
    // What the gaps are if the margins collapse: the larger of the rule's and
    // the paragraph's, never their sum.
    collapsedAbove: Math.max(px(rule, "margin-top"), px(rule.previousElementSibling, "margin-bottom")),
    collapsedBelow: Math.max(px(rule, "margin-bottom"), px(rule.nextElementSibling, "margin-top")),
  };
});

for (const theme of ["dark", "light"]) {
  for (const [host, wrap] of Object.entries(HOSTS)) {
    it(`draws a thematic break as a --line2 hairline with collapsed margins either side, ${host}, ${theme}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await mountLayout(page, `<link rel="stylesheet" href="${basePath}src/styles/tasks.css">${wrap(markdownHtml(RULED))}`,
          { basePath, styles: "body { padding:24px; width:560px; }" });
        await page.evaluate((name) => { document.documentElement.dataset.theme = name; }, theme);
        const rule = await measureRule(page);
        expect(rule.className).toBe("md-rule");
        expect(rule.top).toEqual({ width: "1px", style: "solid", color: rule.line2 });
        for (const other of rule.others) {
          expect(other.width === "0px" || other.style === "none").toBe(true);
        }
        expect(rule.contentHeight).toBe(0);
        expect(rule.boxHeight).toBe(1);
        expect(rule.margins).toEqual(["12px", "12px"]);
        // A chat paragraph's margin is under 12px, so the rule's sets the gap;
        // a task comment's paragraphs keep the browser's 1em, which wins.
        expect(Math.abs(rule.gapAbove - rule.collapsedAbove)).toBeLessThanOrEqual(1);
        expect(Math.abs(rule.gapBelow - rule.collapsedBelow)).toBeLessThanOrEqual(1);
        expect(rule.collapsedAbove).toBe(GAP[host]);
        expect(rule.collapsedBelow).toBe(GAP[host]);
        await captureLayout(page, `md-rule-${host.replace(/\W+/g, "-")}-${theme}.png`);
      });
    });
  }
}
