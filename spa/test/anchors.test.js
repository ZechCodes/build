import { describe, it, expect } from "vitest";
import { slugifyHeading, lineRangeSuffix, anchorLocationLabel } from "../src/core/anchors.js";

describe("slugifyHeading", () => {
  it("lowercases and collapses punctuation runs to single dashes", () => {
    expect(slugifyHeading("Database Schema")).toBe("database-schema");
    expect(slugifyHeading("API endpoints (v2)!")).toBe("api-endpoints-v2");
  });
  it("strips backticks and bold markers before slugging", () => {
    expect(slugifyHeading("The `users` table")).toBe("the-users-table");
    expect(slugifyHeading("**Important** note")).toBe("important-note");
  });
  it("trims leading and trailing dashes", () => {
    expect(slugifyHeading("  spaced  ")).toBe("spaced");
    expect(slugifyHeading("--- edge ---")).toBe("edge");
  });
  it("returns empty for empty / non-alphanumeric / nullish input", () => {
    expect(slugifyHeading("")).toBe("");
    expect(slugifyHeading("!!!")).toBe("");
    expect(slugifyHeading(null)).toBe("");
    expect(slugifyHeading(undefined)).toBe("");
  });
});

describe("lineRangeSuffix", () => {
  it("writes a single line and a span", () => {
    expect(lineRangeSuffix(12, 12)).toBe(":12");
    expect(lineRangeSuffix(12, 18)).toBe(":12-18");
  });
  it("treats a missing end as the start", () => {
    expect(lineRangeSuffix(9, 0)).toBe(":9");
    expect(lineRangeSuffix(9, undefined)).toBe(":9");
  });
  it("prints nothing for an anchor with no range", () => {
    expect(lineRangeSuffix(0, 0)).toBe("");
    expect(lineRangeSuffix(undefined, undefined)).toBe("");
  });
});

describe("anchorLocationLabel", () => {
  it("names the enclosing heading chain and the lines", () => {
    expect(anchorLocationLabel({ heading_path: ["Plan", "Schema"], line_start: 12, line_end: 18 })).toBe(
      "Plan > Schema:12-18",
    );
  });
  it("falls back to the artifact's path when no heading encloses the passage", () => {
    expect(anchorLocationLabel({ heading_path: [], path: "docs/stage-1.md", line_start: 4, line_end: 4 })).toBe(
      "docs/stage-1.md:4",
    );
  });
  it("says top of doc when there is neither a heading nor a path", () => {
    expect(anchorLocationLabel({ heading_path: [] })).toBe("(top of doc)");
  });
  it("calls an unanchored message general", () => {
    expect(anchorLocationLabel(null)).toBe("(general)");
  });
});
