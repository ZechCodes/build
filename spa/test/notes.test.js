import { describe, it, expect } from "vitest";
import { assemblePlanNotes, assembleDiffNotes } from "../src/core/notes.js";

describe("assemblePlanNotes", () => {
  it("numbers each passage comment and collapses whitespace in snippets", () => {
    const notes = assemblePlanNotes(
      [
        { snippet: "  add   a\nhealth endpoint  ", comment: "make it /healthz" },
        { snippet: "use sqlite", comment: "postgres instead" },
      ],
      "",
    );
    expect(notes).toContain("Please revise the plan");
    expect(notes).toContain('1. On the passage: "add a health endpoint"');
    expect(notes).toContain("Comment: make it /healthz");
    expect(notes).toContain('2. On the passage: "use sqlite"');
    expect(notes).not.toContain("General feedback");
  });

  it("appends trimmed general feedback when present", () => {
    const notes = assemblePlanNotes([], "  looks wrong overall  ");
    expect(notes).toContain("General feedback: looks wrong overall");
  });
});

describe("assembleDiffNotes", () => {
  it("labels single lines and ranges, quoting the snippet", () => {
    const notes = assembleDiffNotes(
      [
        { file: "app.py", lnA: 4, lnB: 4, snippet: "x = 1", comment: "rename x" },
        { file: "lib.py", lnA: 2, lnB: 5, snippet: "a\nb", comment: "extract fn" },
      ],
      "",
    );
    expect(notes).toContain("Please make these changes");
    expect(notes).toContain("1. app.py (line 4):");
    expect(notes).toContain("> x = 1");
    expect(notes).toContain("2. lib.py (lines 2-5):");
    expect(notes).toContain("> a\n> b");
    expect(notes).toContain("Comment: extract fn");
  });

  it("appends general feedback when present", () => {
    expect(assembleDiffNotes([], "ship it smaller")).toContain(
      "General feedback: ship it smaller",
    );
  });
});
