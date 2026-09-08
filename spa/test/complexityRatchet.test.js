// The eslint complexity ratchet only goes down (CLAUDE.md "Complexity gates").
//
// `// eslint-disable-next-line complexity` is how a function that was already
// over the cap when the gate landed stays in the tree. eslint is happy either
// way once the comment is written, so the count is asserted here: adding one
// fails this test, and retiring one is a deliberate edit of the number below.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, it, expect } from "vitest";

// Measured when the gate landed: 78 functions across 39 files under src/.
// 77 since diffFileHtml became a header, a body and a frame.
// 75 since the timeline became keyed rows: threadHtml took its initial message
// and its title from two named helpers, and the row builder split into a
// message row and an activity row.
// 73 since chat state moved behind addressed controllers: agent identity no
// longer migrates global draft maps, and message rendering reads its owned
// thread state instead of branching across ambient maps.
const RATCHETED_FUNCTIONS = 73;

const SRC = fileURLToPath(new URL("../src", import.meta.url));
const DISABLE = "eslint-disable-next-line complexity";
// A block or file-level disable would switch the rule off for everything
// below it without touching the count above: none may exist.
const BLANKET = /eslint-disable(?!-next-line)[^\n]*complexity/;

function jsFiles(dir) {
  return readdirSync(dir, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) return jsFiles(path);
      return entry.name.endsWith(".js") ? [path] : [];
    });
}

describe("the complexity ratchet", () => {
  it("is never switched off for a whole block or file", () => {
    const blanket = jsFiles(SRC).filter((path) => BLANKET.test(readFileSync(path, "utf8")));
    expect(blanket, "a blanket eslint-disable for complexity defeats the ratchet").toEqual([]);
  });

  it("holds at the count measured when the gate landed", () => {
    const found = jsFiles(SRC).flatMap((path) =>
      readFileSync(path, "utf8")
        .split("\n")
        .flatMap((line, index) =>
          line.includes(DISABLE) ? [`${path.slice(SRC.length + 1)}:${index + 1}`] : [],
        ),
    );
    expect(
      found.length,
      "the complexity ratchet moved — split the function instead of disabling the " +
        "rule, or lower RATCHETED_FUNCTIONS when retiring one",
    ).toBe(RATCHETED_FUNCTIONS);
  });
});
