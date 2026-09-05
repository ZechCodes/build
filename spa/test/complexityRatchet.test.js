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
const RATCHETED_FUNCTIONS = 78;

const SRC = fileURLToPath(new URL("../src", import.meta.url));
const DISABLE = "eslint-disable-next-line complexity";

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
