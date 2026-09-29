import { describe, it, expect } from "vitest";
import { diffCommentAnchor, diffThreadMessages } from "../src/core/notes.js";

describe("diffCommentAnchor", () => {
  it("names the artifact, path, side and line span of the passage", () => {
    expect(diffCommentAnchor({ file: "src/a.js", lnA: 2, lnB: 4, snippet: "  old()  ", side: "old" }, "diff-r1")).toEqual({
      artifact: "diff",
      revision_id: "diff-r1",
      path: "src/a.js",
      side: "old",
      line_start: 2,
      line_end: 4,
      heading_path: [],
      snippet: "old()",
    });
  });

  it("defaults to the new side and to a single-line span", () => {
    const anchor = diffCommentAnchor({ file: "a.js", lnA: 7, lnB: 0, snippet: "x = 1" });
    expect(anchor.side).toBe("new");
    expect(anchor.revision_id).toBe(null);
    expect(anchor).toMatchObject({ line_start: 7, line_end: 7 });
  });

  it("gives a whole-file comment no line range at all", () => {
    const anchor = diffCommentAnchor({ file: "a.js", lnA: 0, lnB: 0, snippet: "(entire file)" });
    expect(anchor.line_start).toBeUndefined();
    expect(anchor.line_end).toBeUndefined();
    expect(anchor.path).toBe("a.js");
  });

  it("caps the quoted passage at 400 characters", () => {
    expect(diffCommentAnchor({ file: "a.js", lnA: 1, lnB: 1, snippet: "x".repeat(900) }).snippet).toHaveLength(400);
  });
});


describe("diffThreadMessages", () => {
  it("anchors each comment and leaves the general note unanchored", () => {
    expect(
      diffThreadMessages([{ file: "src/a.js", lnA: 2, lnB: 4, snippet: "old()", comment: " rename " }], "ship safely", "diff-r1"),
    ).toEqual([
      {
        body: "rename",
        anchor: {
          artifact: "diff",
          revision_id: "diff-r1",
          path: "src/a.js",
          side: "new",
          line_start: 2,
          line_end: 4,
          heading_path: [],
          snippet: "old()",
        },
      },
      { body: "ship safely", anchor: null },
    ]);
  });

  it("sends nothing at all when there is nothing to say", () => {
    expect(diffThreadMessages([], "   ", null)).toEqual([]);
  });
});
