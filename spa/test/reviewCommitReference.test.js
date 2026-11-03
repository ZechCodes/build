import { describe, expect, it } from "vitest";
import { resolveReviewCommitReference } from "../src/core/reviewCommitReference.js";

const oid = `abcd${"1".repeat(36)}`;
const selected = { revision: 3, mode: "commit", commit: "abcd", directoryId: "dir:root", path: "a.js", side: "new", line: 42 };
const complete = (commits) => ({ commits: commits.map((oid) => ({ oid })) });

describe("a review commit prefix", () => {
  it("resolves a unique case-insensitive prefix to the full cached OID", () => {
    expect(resolveReviewCommitReference({ ...selected, commit: "ABCD" }, complete([oid])))
      .toEqual({ selection: { ...selected, commit: oid } });
  });
  it("defers absent, failed and incomplete commit lists without selecting a prefix", () => {
    for (const commits of [null, {}, { read_error: "unavailable" }, { ...complete([oid]), commits_truncated: true }, { ...complete([oid]), next_cursor: "more" }]) {
      expect(resolveReviewCommitReference(selected, commits)).toEqual({ selection: selected, pending: true });
    }
  });
  it("uses a complete cached list when its refresh failed", () => {
    expect(resolveReviewCommitReference(selected, { ...complete([oid]), read_error: "offline" }))
      .toEqual({ selection: { ...selected, commit: oid } });
  });
  it("falls back to All changes for ambiguous and missing prefixes", () => {
    expect(resolveReviewCommitReference(selected, complete([oid, `abcd${"2".repeat(36)}`])))
      .toEqual({ selection: { ...selected, mode: "all", commit: null }, missing: "ambiguous" });
    expect(resolveReviewCommitReference(selected, complete(["e".repeat(40)])))
      .toEqual({ selection: { ...selected, mode: "all", commit: null }, missing: "missing" });
  });
  it("retains full-OID selections and non-commit comparisons unchanged", () => {
    for (const selection of [{ ...selected, commit: oid }, { ...selected, mode: "all" }]) {
      expect(resolveReviewCommitReference(selection, null)).toEqual({ selection });
    }
  });
  it("does not resolve before an ambiguous file reference supplies the directory", () => {
    const selection = { ...selected, fileRef: "dir:root:a.js" };
    expect(resolveReviewCommitReference(selection, complete([oid]))).toEqual({ selection, pending: true });
  });
});
