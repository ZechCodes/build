import { describe, it, expect } from "vitest";
import { diffCommentAnchor, docCommentAnchor, diffThreadMessages } from "../src/core/notes.js";

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

describe("docCommentAnchor", () => {
  it("carries the heading chain, the passage, and its source lines", () => {
    expect(
      docCommentAnchor({ headingPath: ["Plan", "Schema"], snippet: " use sqlite ", lineStart: 12, lineEnd: 18 }),
    ).toEqual({ heading_path: ["Plan", "Schema"], snippet: "use sqlite", line_start: 12, line_end: 18 });
  });

  it("omits the range when the passage could not be found in the source", () => {
    const anchor = docCommentAnchor({ headingPath: [], snippet: "bolded away", lineStart: 0, lineEnd: 0 });
    expect(anchor).toEqual({ heading_path: [], snippet: "bolded away" });
  });

  it("collapses a one-line passage to the same start and end", () => {
    expect(docCommentAnchor({ headingPath: ["A"], snippet: "s", lineStart: 5 })).toMatchObject({
      line_start: 5,
      line_end: 5,
    });
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
