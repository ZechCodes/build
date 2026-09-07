import { describe, it, expect } from "vitest";
import {
  hashText,
  hashFileRows,
  stampReview,
  changedSinceReview,
  stampChangeset,
  changedSinceChangeset,
  changesetStamped,
} from "../src/core/reviewMemory.js";

const fileA = { path: "a.js", rows: [{ text: "line 1" }, { text: "line 2" }] };
const fileB = { path: "b.js", rows: [{ text: "hello" }] };
const fileAEdited = { path: "a.js", rows: [{ text: "line 1" }, { text: "line CHANGED" }] };

// A file view is what the stack draws and what a stamp is taken of: a path and
// the content key of what it holds. An uncommitted file wears the bridge's key,
// a parsed one a hash of its rows (core/fileEntries.js makes both).
const viewOf = (file) => ({ path: file.path, contentKey: hashFileRows(file) });
const viewAt = (path, contentKey) => ({ path, contentKey });

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
    const stamps = stampReview([viewOf(fileA), viewOf(fileB)]);
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
    const stamps = stampReview([viewOf(fileA), viewOf(fileB)]);
    expect(changedSinceReview(stamps, [viewOf(fileA), viewOf(fileB)]).size).toBe(0);
  });
  it("flags a file that was edited since the stamp", () => {
    const stamps = stampReview([viewOf(fileA), viewOf(fileB)]);
    const changed = changedSinceReview(stamps, [viewOf(fileAEdited), viewOf(fileB)]);
    expect(changed.has("a.js")).toBe(true);
    expect(changed.has("b.js")).toBe(false);
  });
  it("flags a brand-new file once any stamp exists", () => {
    const stamps = stampReview([viewOf(fileA)]);
    const newFile = { path: "c.js", rows: [{ text: "new" }] };
    const changed = changedSinceReview(stamps, [viewOf(fileA), viewOf(newFile)]);
    expect(changed.has("c.js")).toBe(true);
    expect(changed.has("a.js")).toBe(false);
  });
  it("flags nothing when the stamp is empty (never reviewed)", () => {
    expect(changedSinceReview(new Map(), [viewOf(fileA), viewOf(fileB)]).size).toBe(0);
    expect(changedSinceReview(null, [viewOf(fileA)]).size).toBe(0);
  });
});

describe("per-changeset review memory", () => {
  const fileAt = (path, contentKey) => viewAt(path, contentKey);

  it("keeps each changeset's baseline apart", () => {
    let stamps = new Map();
    stamps = stampChangeset(stamps, "uncommitted", [fileAt("a.js", "one")]);
    stamps = stampChangeset(stamps, "abc123", [fileAt("b.js", "two")]);
    expect(changedSinceChangeset(stamps, "uncommitted", [fileAt("a.js", "one")]).size).toBe(0);
    expect([...changedSinceChangeset(stamps, "uncommitted", [fileAt("a.js", "moved")])]).toEqual(["a.js"]);
    // b.js is the other changeset's file: reviewing it there says nothing here.
    expect([...changedSinceChangeset(stamps, "uncommitted", [fileAt("b.js", "two")])]).toEqual(["b.js"]);
  });

  it("flags nothing on a changeset that was never reviewed", () => {
    const stamps = stampChangeset(new Map(), "uncommitted", [fileAt("a.js", "one")]);
    expect(changedSinceChangeset(stamps, "abc123", [fileAt("a.js", "one")]).size).toBe(0);
    expect(changesetStamped(stamps, "abc123")).toBe(false);
    expect(changesetStamped(stamps, "uncommitted")).toBe(true);
  });

  it("does not mutate the map it was given", () => {
    const stamps = new Map();
    stampChangeset(stamps, "uncommitted", [fileAt("a.js", "one")]);
    expect(stamps.size).toBe(0);
  });

  it("counts an empty changeset as no baseline at all", () => {
    expect(changesetStamped(stampChangeset(new Map(), "uncommitted", []), "uncommitted")).toBe(false);
  });
});
