import { describe, it, expect } from "vitest";
import { resolveLegacyRoute } from "../src/core/routeResolve.js";

// One feed's worth of rows in the new items[] shape (bridge board.list).
const items = [
  { kind: "branch", project_id: "p1", branch: "main", primary: true, worktree_id: "wt-main", run_id: null, issue_id: null },
  { kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-1", worktree_id: "wt-1", issue_id: "issue-9" },
  { kind: "branch", project_id: "p2", branch: "detached-head", run_id: "run-2", worktree_id: "wt-2", issue_id: null },
  { kind: "issue", project_id: "p2", branch: null, issue_id: "issue-3", run_id: null, worktree_id: null },
];

describe("resolveLegacyRoute", () => {
  it("resolves a run id to its branch, carrying the canonical tab", () => {
    expect(resolveLegacyRoute({ kind: "run", id: "run-1", tab: "files" }, items)).toEqual({
      name: "branch", projectId: "p1", branch: "build/login", tab: "files",
    });
    expect(resolveLegacyRoute({ kind: "run", id: "run-2" }, items)).toEqual({
      name: "branch", projectId: "p2", branch: "detached-head", tab: "changes",
    });
  });

  it("resolves a worktree id to its branch", () => {
    expect(resolveLegacyRoute({ kind: "worktree", id: "wt-1", tab: "changes" }, items)).toEqual({
      name: "branch", projectId: "p1", branch: "build/login", tab: "changes",
    });
  });

  it("resolves a project's primary checkout to its branch row", () => {
    expect(resolveLegacyRoute({ kind: "primary", projectId: "p1", tab: "files" }, items)).toEqual({
      name: "branch", projectId: "p1", branch: "main", tab: "files",
    });
  });

  it("resolves an issue id to its issue, keeping the stage deep-link", () => {
    expect(resolveLegacyRoute({ kind: "issue", id: "issue-3" }, items)).toEqual({
      name: "issue", projectId: "p2", id: "issue-3",
    });
    expect(resolveLegacyRoute({ kind: "issue", id: "issue-3", stage: "s2" }, items)).toEqual({
      name: "issue", projectId: "p2", id: "issue-3", stage: "s2",
    });
  });

  // Dedup: an issue whose implementation is in flight has no row of its own —
  // its branch row carries the issue id. The branch IS the nearest surface.
  it("resolves an issue being implemented to the branch that carries it", () => {
    expect(resolveLegacyRoute({ kind: "issue", id: "issue-9", stage: "s1" }, items)).toEqual({
      name: "branch", projectId: "p1", branch: "build/login", tab: "changes",
    });
  });

  it("prefers the row in the project the URL named", () => {
    const ambiguous = [
      { kind: "branch", project_id: "p2", branch: "same-name", worktree_id: "wt-x" },
      { kind: "branch", project_id: "p1", branch: "the-one", worktree_id: "wt-x" },
    ];
    expect(resolveLegacyRoute({ kind: "worktree", projectId: "p1", id: "wt-x" }, ambiguous)).toEqual({
      name: "branch", projectId: "p1", branch: "the-one", tab: "changes",
    });
  });

  it("answers null when nothing in the feed carries that id", () => {
    expect(resolveLegacyRoute({ kind: "run", id: "gone" }, items)).toBeNull();
    expect(resolveLegacyRoute({ kind: "primary", projectId: "p9" }, items)).toBeNull();
    expect(resolveLegacyRoute({ kind: "issue", id: "nope" }, items)).toBeNull();
    expect(resolveLegacyRoute({ kind: "run", id: "run-1" }, [])).toBeNull();
    expect(resolveLegacyRoute({ kind: "run", id: "run-1" }, undefined)).toBeNull();
  });

  // A row with no branch name cannot address a branch URL; the inbox is where
  // an unaddressable entity belongs.
  it("answers null for a row that has no branch to open", () => {
    const nameless = [{ kind: "branch", project_id: "p1", branch: null, run_id: "run-7" }];
    expect(resolveLegacyRoute({ kind: "run", id: "run-7" }, nameless)).toBeNull();
  });

  it("answers null for a ref it has no rule for", () => {
    expect(resolveLegacyRoute({ kind: "mystery", id: "x" }, items)).toBeNull();
    expect(resolveLegacyRoute(null, items)).toBeNull();
  });
});
