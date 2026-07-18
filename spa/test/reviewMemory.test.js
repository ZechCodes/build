import { describe, it, expect } from "vitest";
import { hashText, hashFileRows, stampReview, changedSinceReview } from "../src/core/reviewMemory.js";

const fileA = { path: "a.js", rows: [{ text: "line 1" }, { text: "line 2" }] };
const fileB = { path: "b.js", rows: [{ text: "hello" }] };
const fileAEdited = { path: "a.js", rows: [{ text: "line 1" }, { text: "line CHANGED" }] };

describe("hashText", () => {
  it("is stable for the same input", () => {
    expect(hashText("abc")).toBe(hashText("abc"));
  });
  it("differs for different input", () => {
    expect(hashText("abc")).not.toBe(hashText("abd"));
  });
  it("returns a hex string and is safe for empty/absent input", () => {
    expect(hashText("anything")).toMatch(/^[0-9a-f]+$/);
    expect(hashText("")).toMatch(/^[0-9a-f]+$/);
    expect(hashText(undefined)).toMatch(/^[0-9a-f]+$/);
  });
});

describe("hashFileRows", () => {
  it("is stable across calls for the same rows", () => {
    expect(hashFileRows(fileA)).toBe(hashFileRows(fileA));
  });
  it("changes when a row's text changes", () => {
    expect(hashFileRows(fileA)).not.toBe(hashFileRows(fileAEdited));
  });
  it("is safe for a file with no rows", () => {
    expect(typeof hashFileRows({ path: "x", rows: [] })).toBe("string");
    expect(typeof hashFileRows({})).toBe("string");
  });
});

describe("stampReview", () => {
  it("maps every file path to its row hash", () => {
    const stamps = stampReview([fileA, fileB]);
    expect(stamps.get("a.js")).toBe(hashFileRows(fileA));
    expect(stamps.get("b.js")).toBe(hashFileRows(fileB));
    expect(stamps.size).toBe(2);
  });
  it("is safe for an absent list", () => {
    expect(stampReview(undefined).size).toBe(0);
  });
});

describe("changedSinceReview", () => {
  it("flags nothing when the files are identical to the stamp", () => {
    const stamps = stampReview([fileA, fileB]);
    expect(changedSinceReview(stamps, [fileA, fileB]).size).toBe(0);
  });
  it("flags a file that was edited since the stamp", () => {
    const stamps = stampReview([fileA, fileB]);
    const changed = changedSinceReview(stamps, [fileAEdited, fileB]);
    expect(changed.has("a.js")).toBe(true);
    expect(changed.has("b.js")).toBe(false);
  });
  it("flags a brand-new file once any stamp exists", () => {
    const stamps = stampReview([fileA]);
    const newFile = { path: "c.js", rows: [{ text: "new" }] };
    const changed = changedSinceReview(stamps, [fileA, newFile]);
    expect(changed.has("c.js")).toBe(true);
    expect(changed.has("a.js")).toBe(false);
  });
  it("flags nothing when the stamp is empty (never reviewed)", () => {
    expect(changedSinceReview(new Map(), [fileA, fileB]).size).toBe(0);
    expect(changedSinceReview(null, [fileA]).size).toBe(0);
  });
});
