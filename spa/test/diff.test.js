import { describe, it, expect } from "vitest";
import { createFileFolds, fileKey, firstLineOf, parseDiff } from "../src/core/diff.js";

const SAMPLE_PATCH = `diff --git a/greeting.py b/greeting.py
new file mode 100644
index 0000000..e69de29
--- /dev/null
+++ b/greeting.py
@@ -0,0 +1,2 @@
+def hello():
+    return "Hello!"
diff --git a/app.py b/app.py
index 1111111..2222222 100644
--- a/app.py
+++ b/app.py
@@ -10,3 +10,4 @@ def main():
 keep_one
-old_line
+new_line
 keep_two
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 3333333..0000000
--- a/gone.txt
+++ /dev/null
@@ -1,1 +0,0 @@
-goodbye`;

describe("parseDiff", () => {
  it("returns one entry per file with status detection", () => {
    const files = parseDiff(SAMPLE_PATCH);
    expect(files.map((f) => f.path)).toEqual(["greeting.py", "app.py", "gone.txt"]);
    expect(files.map((f) => f.status)).toEqual(["ADD", "EDIT", "DEL"]);
  });

  it("counts additions and deletions per file", () => {
    const [added, edited, deleted] = parseDiff(SAMPLE_PATCH);
    expect(added.add).toBe(2);
    expect(added.del).toBe(0);
    expect(edited.add).toBe(1);
    expect(edited.del).toBe(1);
    expect(deleted.del).toBe(1);
  });

  it("tracks old/new line numbers through hunks", () => {
    const edited = parseDiff(SAMPLE_PATCH)[1];
    const rows = edited.rows;
    expect(rows[0]).toEqual({ t: "hunk", text: "@@ -10,3 +10,4 @@ def main():" });
    expect(rows[1]).toEqual({ t: "ctx", o: 10, n: 10, text: "keep_one" });
    expect(rows[2]).toEqual({ t: "del", o: 11, text: "old_line" });
    expect(rows[3]).toEqual({ t: "add", n: 11, text: "new_line" });
    expect(rows[4]).toEqual({ t: "ctx", o: 12, n: 12, text: "keep_two" });
  });

  it("handles empty and missing patches", () => {
    expect(parseDiff("")).toEqual([]);
    expect(parseDiff(null)).toEqual([]);
    expect(parseDiff(undefined)).toEqual([]);
  });

  it("ignores content before the first diff header", () => {
    expect(parseDiff("stray line\n+not a real add\n")).toEqual([]);
  });
});

// ---- file identity + the reader's folds --------------------------------------

describe("fileKey", () => {
  it("names a file by its path and its status", () => {
    const [added, edited, deleted] = parseDiff(SAMPLE_PATCH);
    expect(fileKey(added)).toContain("greeting.py");
    expect(fileKey(edited)).toContain("app.py");
    expect(fileKey(added)).not.toBe(fileKey(edited));
  });

  it("keeps a rename's delete and add apart", () => {
    const gone = { path: "moved.py", status: "DEL" };
    const arrived = { path: "moved.py", status: "ADD" };
    expect(fileKey(gone)).not.toBe(fileKey(arrived));
  });
});

describe("createFileFolds", () => {
  it("starts with every file capped — neither opened nor shut", () => {
    const folds = createFileFolds();
    expect(folds.expanded.size).toBe(0);
    expect(folds.collapsed.size).toBe(0);
  });

  it("opens a file the reader pressed the body of", () => {
    const folds = createFileFolds();
    folds.open("EDIT:a.js");
    expect(folds.expanded.has("EDIT:a.js")).toBe(true);
    expect(folds.collapsed.has("EDIT:a.js")).toBe(false);
  });

  it("shuts an open file on the head press, and opens a shut one", () => {
    const folds = createFileFolds();
    folds.open("EDIT:a.js");
    folds.pressedHead("EDIT:a.js");
    expect(folds.collapsed.has("EDIT:a.js")).toBe(true);
    expect(folds.expanded.has("EDIT:a.js")).toBe(false);
    folds.pressedHead("EDIT:a.js");
    expect(folds.expanded.has("EDIT:a.js")).toBe(true);
    expect(folds.collapsed.has("EDIT:a.js")).toBe(false);
  });

  it("shuts a capped file on the head press", () => {
    const folds = createFileFolds();
    folds.pressedHead("EDIT:a.js");
    expect(folds.collapsed.has("EDIT:a.js")).toBe(true);
  });
});

describe("firstLineOf", () => {
  it("names the first line of the file the diff touches", () => {
    const [added, edited] = parseDiff(SAMPLE_PATCH);
    expect(firstLineOf(edited)).toBe(10);
    expect(firstLineOf(added)).toBe(1);
  });

  it("falls back to the top of the file when the diff names no new line", () => {
    expect(firstLineOf({ rows: [] })).toBe(1);
    expect(firstLineOf({ rows: [{ t: "del", o: 4, text: "gone" }] })).toBe(1);
  });
});
