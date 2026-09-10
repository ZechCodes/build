import { describe, expect, it } from "vitest";
import { createParsedDiffCache, filePatches } from "../src/core/parsedDiffCache.js";

const file = (path, text) => `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -1 +1 @@\n-old\n+${text}\n`;

describe("incremental parsed diffs", () => {
  it("splits an aggregate patch without losing file boundaries", () => {
    expect(filePatches(file("a.js", "a") + file("b.js", "b"))).toHaveLength(2);
  });

  it("reuses unchanged file views and replaces only changed content", () => {
    const cache = createParsedDiffCache();
    const first = cache.views(file("a.js", "a") + file("b.js", "b"));
    const second = cache.views(file("a.js", "a") + file("b.js", "new"));
    expect(second[0].rows).toBe(first[0].rows);
    expect(second[1].rows).not.toBe(first[1].rows);
  });

  it("uses supplied content revisions and applies fresh timestamps", () => {
    const cache = createParsedDiffCache();
    const patch = file("a.js", "a");
    const first = cache.views(patch, { contentKeys: { "a.js": "one" }, editedAt: { "a.js": 10 } });
    const second = cache.views(patch, { contentKeys: { "a.js": "one" }, editedAt: { "a.js": 20 } });
    expect(second[0].rows).toBe(first[0].rows);
    expect(second[0].editedAt).toBe(20);
  });

  it("evicts the least recently used parse after reaching its bound", () => {
    const cache = createParsedDiffCache({ limit: 2 });
    const firstA = cache.views(file("a.js", "a"))[0];
    const firstB = cache.views(file("b.js", "b"))[0];
    expect(cache.views(file("a.js", "a"))[0].rows).toBe(firstA.rows); // a is newest
    cache.views(file("c.js", "c"));
    expect(cache.views(file("b.js", "b"))[0].rows).not.toBe(firstB.rows);
  });
});
