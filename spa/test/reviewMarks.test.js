// What the reviewer has marked on the files of a changeset, and what a commit
// does about it.

import { describe, expect, it } from "vitest";
import { commitPaths, createReviewMarks } from "../src/core/reviewMarks.js";

describe("createReviewMarks", () => {
  it("starts with nothing marked", () => {
    const marks = createReviewMarks();
    expect([...marks.approved]).toEqual([]);
    expect([...marks.selected]).toEqual([]);
  });

  it("approves a file and takes it back", () => {
    const marks = createReviewMarks();
    marks.toggleApproved("a.js");
    expect(marks.approved.has("a.js")).toBe(true);
    marks.toggleApproved("a.js");
    expect(marks.approved.has("a.js")).toBe(false);
  });

  it("selects a file and lets it go", () => {
    const marks = createReviewMarks();
    marks.toggleSelected("a.js");
    expect(marks.selected.has("a.js")).toBe(true);
    marks.toggleSelected("a.js");
    expect(marks.selected.has("a.js")).toBe(false);
  });

  it("approves everything selected in one verb, and drops the selection after", () => {
    const marks = createReviewMarks();
    marks.toggleSelected("a.js");
    marks.toggleSelected("b.js");
    marks.approveSelected();
    expect([...marks.approved].sort()).toEqual(["a.js", "b.js"]);
    expect([...marks.selected]).toEqual([]);
  });

  it("keeps an approval the bulk verb did not touch", () => {
    const marks = createReviewMarks();
    marks.toggleApproved("old.js");
    marks.toggleSelected("a.js");
    marks.approveSelected();
    expect([...marks.approved].sort()).toEqual(["a.js", "old.js"]);
  });

  it("lets a selection go without approving it", () => {
    const marks = createReviewMarks();
    marks.toggleSelected("a.js");
    marks.clearSelection();
    expect([...marks.selected]).toEqual([]);
    expect([...marks.approved]).toEqual([]);
  });
});

describe("commitPaths", () => {
  const changed = ["src/a.js", "src/b.js", "uv.lock"];

  it("takes the whole worktree when nothing is ticked", () => {
    expect(commitPaths(changed, new Set())).toEqual(changed);
  });

  it("takes only what is ticked", () => {
    expect(commitPaths(changed, new Set(["src/b.js", "uv.lock"]))).toEqual(["src/b.js", "uv.lock"]);
  });

  it("keeps the worktree's own order, not the order they were ticked in", () => {
    expect(commitPaths(changed, new Set(["uv.lock", "src/a.js"]))).toEqual(["src/a.js", "uv.lock"]);
  });

  it("ignores a tick on a file that is no longer changed", () => {
    expect(commitPaths(changed, new Set(["gone.js"]))).toEqual(changed);
  });

  it("answers nothing over a worktree with nothing in it", () => {
    expect(commitPaths([], new Set(["a.js"]))).toEqual([]);
    expect(commitPaths(null, null)).toEqual([]);
  });
});
