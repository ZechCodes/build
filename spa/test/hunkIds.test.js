// Hunk identity is a contract between two languages: the bridge names hunks in
// a triage report, and the SPA has to find those same hunks in the same patch.
// These tests pin the properties the id is supposed to have, and then pin the
// ids themselves against the fixture bridge/src/diff.rs asserts.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { hunkIds, patchHunks } from "../src/core/diff.js";

const FIXTURE = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("../../bridge/tests/fixtures/hunk_ids.json", import.meta.url)),
    "utf8",
  ),
);

describe("patchHunks", () => {
  it("gives every hunk of a patch its own id", () => {
    const patch = FIXTURE.cases.find((c) => c.name === "two files, three hunks").patch;
    const hunks = patchHunks(patch);
    expect(hunks.map((h) => h.path)).toEqual(["greeting.py", "app.py", "app.py"]);
    expect(new Set(hunks.map((h) => h.hunk_id)).size).toBe(3);
    for (const hunk of hunks) {
      expect(hunk.hunk_id).toMatch(/^h[0-9a-f]{12}$/);
    }
  });

  it("keeps a hunk's id when the hunk only moved", () => {
    const before = "diff --git a/app.py b/app.py\n@@ -10,3 +10,4 @@ def main():\n keep_one\n-old_line\n+new_line\n keep_two\n";
    const after = "diff --git a/app.py b/app.py\n@@ -80,3 +91,4 @@ def something_else():\n keep_one\n-old_line\n+new_line\n keep_two\n";
    expect(hunkIds(before)).toEqual(hunkIds(after));
  });

  it("treats another file, another shape, or another body as another hunk", () => {
    const material = (path, header, body) => `diff --git a/${path} b/${path}\n${header}\n${body}`;
    const base = material("app.py", "@@ -1,2 +1,3 @@", " ctx\n+added\n");
    expect(hunkIds(base)).not.toEqual(hunkIds(material("other.py", "@@ -1,2 +1,3 @@", " ctx\n+added\n")));
    expect(hunkIds(base)).not.toEqual(hunkIds(material("app.py", "@@ -1,2 +1,4 @@", " ctx\n+added\n")));
    expect(hunkIds(base)).not.toEqual(hunkIds(material("app.py", "@@ -1,2 +1,3 @@", " ctx\n+different\n")));
  });

  it("gives two identical hunks in one file distinct, stable ids", () => {
    const patch = "diff --git a/app.py b/app.py\n@@ -1,1 +1,2 @@\n ctx\n+added\n@@ -30,1 +31,2 @@\n ctx\n+added\n";
    const ids = hunkIds(patch);
    expect(ids).toHaveLength(2);
    expect(ids[0]).not.toBe(ids[1]);
    expect(hunkIds(patch)).toEqual(ids);
  });

  it("does not count a malformed @@ line as a hunk", () => {
    const patch = "diff --git a/app.py b/app.py\n@@ not a hunk header @@\n ctx\n@@ -1,1 +1,2 @@\n ctx\n+added\n";
    expect(patchHunks(patch).map((h) => h.header)).toEqual(["@@ -1,1 +1,2 @@"]);
  });

  it("produces exactly the ids the bridge produces", () => {
    expect(FIXTURE.cases.length).toBeGreaterThan(0);
    for (const { name, patch, hunks } of FIXTURE.cases) {
      expect(patchHunks(patch), name).toEqual(hunks);
    }
  });
});
