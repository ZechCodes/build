// How a pipe table survives the narrow column it is read in.
//
// A conversation card is a ~340px column, and its body wraps anywhere so a
// pasted path can never widen it. A table cell that inherits that rule has no
// minimum width at all: the browser is free to shrink a column until its text
// runs one character per line, which is what a wide run report did. A cell has
// to hold its longest word, and the table scrolls in its own box when the
// columns no longer add up.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const STYLES = readFileSync(resolve("src/styles.css"), "utf8");

/** The declarations of the one rule whose selector list is exactly `selector`,
 *  as a property → value map. Comments go first so a brace inside prose cannot
 *  be read as a rule, and a rule is taken as the last selector before its
 *  block, which is what an @media-wrapped rule leaves behind. */
function ruleOf(selector) {
  const withoutComments = STYLES.replace(/\/\*[\s\S]*?\*\//g, "");
  const declarations = withoutComments
    .split("}")
    .map((block) => block.split("{"))
    .filter((parts) => parts.length > 1)
    .find((parts) => parts.at(-2).trim().replace(/\s+/g, " ") === selector)
    ?.at(-1);
  if (declarations === undefined) throw new Error(`no rule for "${selector}" in styles.css`);
  return Object.fromEntries(
    declarations
      .split(";")
      .map((declaration) => declaration.trim())
      .filter(Boolean)
      .map((declaration) => {
        const colon = declaration.indexOf(":");
        return [declaration.slice(0, colon).trim(), declaration.slice(colon + 1).trim()];
      }),
  );
}

describe("a markdown table in a narrow column", () => {
  it("wraps its cells on spaces, so a column is never thinner than its longest word", () => {
    const cell = ruleOf(".mdtable th, .mdtable td");
    // break-word, not anywhere: it lets a word that cannot fit on a line of its
    // own break, but it does not let the column be measured that narrow.
    expect(cell["overflow-wrap"]).toBe("break-word");
    expect(cell["word-break"]).toBe("normal");
  });

  it("scrolls the wrapper rather than the message when the columns do not fit", () => {
    expect(ruleOf(".mdtable")["overflow-x"]).toBe("auto");
    expect(ruleOf(".mdtable table")["min-width"]).toBe("100%");
  });

  it("shows an edge shadow on the side there is still table to reach", () => {
    // The shadows are sized by the table's own scroll timeline (#253), not
    // hidden under covers in a guessed host colour: a cover that missed its host
    // drew a band down each edge of a table that did not scroll at all. The
    // pixels are checked in Chromium, on several hosts, by
    // test/browser/markdownTableEdgesLayout.test.js.
    const table = ruleOf(".mdtable");
    expect(table["background"]).not.toMatch(/local|--mdtable-cover/);
    expect(table["background"]).toContain("var(--mdtable-shade-left)");
    expect(table["background"]).toContain("var(--mdtable-shade-right)");
  });

  it("runs the shading animation only where a scroll timeline exists", () => {
    // A browser without scroll-driven animations drops animation-timeline and
    // plays the animation on the document timeline, 0s long, filled at its
    // last keyframe: a 14px left band on every table, fitting ones included.
    // So the animation lives only inside the @supports block that needs it.
    const withoutComments = STYLES.replace(/\/\*[\s\S]*?\*\//g, "");
    const gated = /@supports \(animation-timeline: scroll\(\)\) \{\s*\.mdtable \{\s*animation:mdtable-edges linear both; animation-timeline:scroll\(self inline\); \}\s*\}/;
    expect(withoutComments).toMatch(gated);
    const ungated = withoutComments.replace(gated, "");
    expect(ungated).not.toMatch(/animation(-timeline)?:\s*(mdtable-edges|scroll\()/);
  });

  it("gives a table the conversation card's full width, gutter to gutter", () => {
    // The body's 12px gutter is the message's, not the table's: pulling it back
    // buys 24px of column and makes the scroll run edge to edge.
    const bleed = ruleOf(".thread-body .mdtable");
    expect(bleed["margin-inline"]).toBe("-12px");
    expect(bleed["padding-inline"]).toBe("12px");
    // .mdtable caps itself at 100% of its parent; the bleed has to lift that cap
    // or the negative margins slide the table instead of widening it.
    expect(bleed["max-width"]).toBe("none");
  });
});
