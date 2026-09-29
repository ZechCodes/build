// #253: a comment addressed to the user is marked by one quiet signal — a
// thin accent rule down the card's left edge — and the card is otherwise the
// neutral card every comment is. The tint that filled the whole card made a
// long addressed comment one solid green block.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const STYLES = readFileSync(resolve("src/styles/tasks.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** The declarations of the rule for exactly `selector`, property → value. */
function ruleOf(selector) {
  const block = STYLES.split("}").map((part) => part.split("{")).find((parts) => parts.length === 2 && parts[0].trim() === selector);
  if (!block) throw new Error(`no rule for "${selector}" in tasks.css`);
  return Object.fromEntries(block[1].split(";").map((part) => part.trim()).filter(Boolean)
    .map((part) => [part.slice(0, part.indexOf(":")).trim(), part.slice(part.indexOf(":") + 1).trim()]));
}

describe("a comment addressed to the user", () => {
  const mentioned = ruleOf(".task-comment-mentioned .task-comment-card");

  it("keeps the neutral card's background", () => {
    expect(mentioned).not.toHaveProperty("--task-comment-background");
    expect(mentioned).not.toHaveProperty("background");
  });

  it("is marked by a 2px accent rule on the left, the text kept where every comment's is", () => {
    expect(mentioned["border-left"]).toBe("2px solid var(--accent)");
    // 1px border + 12px padding on a plain card; 2px + 11px here.
    expect(mentioned["padding-left"]).toBe("11px");
  });
});
