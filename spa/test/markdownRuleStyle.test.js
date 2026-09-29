// #253: a thematic break is drawn as the app's own divider — a hairline in the
// card-border token with the spacing of the prose around it — not the
// browser's default inset, two-tone line.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const STYLES = readFileSync(resolve("src/styles.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** The declarations of the rule for exactly `selector`, property → value. */
function ruleOf(selector) {
  const block = STYLES.split("}").map((part) => part.split("{")).find((parts) => parts.length === 2 && parts[0].trim() === selector);
  if (!block) throw new Error(`no rule for "${selector}" in styles.css`);
  return Object.fromEntries(block[1].split(";").map((part) => part.trim()).filter(Boolean)
    .map((part) => [part.slice(0, part.indexOf(":")).trim(), part.slice(part.indexOf(":") + 1).trim()]));
}

describe("a markdown rule", () => {
  it("is one hairline in the card-border token, with no browser inset", () => {
    const rule = ruleOf(".md-rule");
    expect(rule.border).toBe("0");
    expect(rule["border-top"]).toBe("1px solid var(--line2)");
    expect(rule.height).toBe("0");
  });

  it("keeps the vertical rhythm of the paragraphs around it", () => {
    expect(ruleOf(".md-rule").margin).toBe("12px 0");
  });
});
