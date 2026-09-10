import { describe, it, expect } from "vitest";
import { fuzzyScore, fuzzyRank } from "../src/core/fuzzy.js";

const BRANCHES = [
  "main",
  "build/worktree",
  "feat/rules-command-and-sus",
  "feat/core-moderation-parity",
  "eval/concise-chat-prompt",
  "fix/disboard-bump-notifications",
  "feat/core-parity",
];

describe("fuzzyScore", () => {
  it("matches a scattered subsequence and rejects a non-subsequence", () => {
    expect(fuzzyScore("feat/core-moderation-parity", "fcm")).not.toBeNull();
    expect(fuzzyScore("feat/core-moderation-parity", "zzz")).toBeNull();
  });

  it("is case-insensitive", () => {
    expect(fuzzyScore("feat/Core-Moderation", "core")).not.toBeNull();
  });

  it("ranks exact above prefix above scattered", () => {
    const exact = fuzzyScore("main", "main");
    const prefix = fuzzyScore("main-thing", "main");
    const scattered = fuzzyScore("feat/my-admin-interface", "main");
    expect(exact).toBeGreaterThan(prefix);
    expect(prefix).toBeGreaterThan(scattered);
  });

  it("treats an empty query as a non-filter", () => {
    expect(fuzzyScore("anything", "")).toBe(0);
    expect(fuzzyScore("anything", "   ")).toBe(0);
  });
});

describe("fuzzyRank", () => {
  it("keeps the caller's order when there is no query (branches arrive newest first)", () => {
    expect(fuzzyRank(BRANCHES, "")).toEqual(BRANCHES);
  });

  it("drops non-matches and puts the closest first", () => {
    const ranked = fuzzyRank(BRANCHES, "core");
    expect(ranked).toEqual(["feat/core-moderation-parity", "feat/core-parity"]);
  });

  it("finds a branch from initials across separators", () => {
    expect(fuzzyRank(BRANCHES, "fdbn")[0]).toBe("fix/disboard-bump-notifications");
  });

  it("puts an exact name first even when others also match", () => {
    expect(fuzzyRank([...BRANCHES, "mainline"], "main")[0]).toBe("main");
  });

  it("breaks ties by the caller's order, not alphabetically", () => {
    const ranked = fuzzyRank(["b/thing", "a/thing"], "thing");
    expect(ranked).toEqual(["b/thing", "a/thing"]);
  });

  it("reads the match text through `key` for object items", () => {
    const items = [{ name: "feat/core-parity" }, { name: "main" }];
    expect(fuzzyRank(items, "core", (b) => b.name)).toEqual([{ name: "feat/core-parity" }]);
  });
});
