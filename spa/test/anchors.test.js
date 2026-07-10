import { describe, it, expect } from "vitest";
import { slugifyHeading, buildHeadingPath } from "../src/core/anchors.js";

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

describe("buildHeadingPath", () => {
  it("returns [] when no heading precedes the anchor", () => {
    expect(buildHeadingPath([])).toEqual([]);
  });
  it("chains an h2 under its enclosing h1", () => {
    expect(buildHeadingPath([{ level: 1, text: "A" }, { level: 2, text: "B" }])).toEqual(["A", "B"]);
  });
  it("chains h3 under h2 under h1, outermost first", () => {
    expect(
      buildHeadingPath([{ level: 1, text: "A" }, { level: 2, text: "B" }, { level: 3, text: "C" }]),
    ).toEqual(["A", "B", "C"]);
  });
  it("picks the nearest sibling and skips earlier same-level headings", () => {
    expect(
      buildHeadingPath([{ level: 1, text: "A" }, { level: 2, text: "B" }, { level: 2, text: "C" }]),
    ).toEqual(["A", "C"]);
  });
  it("returns just the top heading when only an h1 precedes", () => {
    expect(buildHeadingPath([{ level: 1, text: "Only" }])).toEqual(["Only"]);
  });
  it("starts from the nearest heading when no enclosing h1 exists", () => {
    expect(buildHeadingPath([{ level: 2, text: "B" }, { level: 3, text: "C" }])).toEqual(["B", "C"]);
  });
});
