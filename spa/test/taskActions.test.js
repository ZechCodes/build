import { describe, it, expect } from "vitest";
import { mergeFailureReason, gitActionConfirm, abandonConfirm } from "../src/core/taskActions.js";

describe("git-action confirmation plans (the modal's step outline)", () => {
  const names = { branch: "feat/x", base: "main" };

  it("merge & clean up outlines commit, merge, worktree removal, and branch deletion", () => {
    expect(gitActionConfirm("merge_prune", names)).toEqual({
      title: "Merge feat/x?",
      actions: [
        "Commit any uncommitted changes on feat/x",
        "Merge feat/x into main",
        "Delete the worktree",
        "Delete branch feat/x",
      ],
      confirmLabel: "Merge & clean up",
      danger: true,
    });
  });

  it("merge & keep outlines commit + merge and keeps the worktree (not danger)", () => {
    expect(gitActionConfirm("merge_keep", names)).toEqual({
      title: "Merge feat/x?",
      actions: [
        "Commit any uncommitted changes on feat/x",
        "Merge feat/x into main",
        "Keep the worktree and branch",
      ],
      confirmLabel: "Merge",
      danger: false,
    });
  });

  it("merge & release outlines the un-adopt step (not danger)", () => {
    expect(gitActionConfirm("merge_release", names)).toEqual({
      title: "Merge feat/x?",
      actions: [
        "Commit any uncommitted changes on feat/x",
        "Merge feat/x into main",
        "Release the task — keep the worktree and branch, drop the task",
      ],
      confirmLabel: "Merge & release",
      danger: false,
    });
  });

  it("merge & push appends the origin push to the clean-up steps", () => {
    expect(gitActionConfirm("merge_push", names)).toEqual({
      title: "Merge feat/x?",
      actions: [
        "Commit any uncommitted changes on feat/x",
        "Merge feat/x into main",
        "Delete the worktree",
        "Delete branch feat/x",
        "Push main to origin",
      ],
      confirmLabel: "Merge & push",
      danger: true,
    });
  });

  it("non-destructive actions (commit, push) get NO modal", () => {
    expect(gitActionConfirm("commit", names)).toBeNull();
    expect(gitActionConfirm("push", names)).toBeNull();
    expect(gitActionConfirm("unknown_option", names)).toBeNull();
  });

  it("interpolates a placeholder base ('the base branch') gracefully", () => {
    const plan = gitActionConfirm("merge_push", { branch: "feat/x", base: "the base branch" });
    expect(plan.actions).toContain("Merge feat/x into the base branch");
    expect(plan.actions).toContain("Push the base branch to origin");
  });
});

describe("abandon confirmation plan", () => {
  it("an adopted worktree warns that files Build did not create are deleted", () => {
    expect(abandonConfirm({ adopted: true, branch: "feat/x" })).toEqual({
      title: "Delete this adopted worktree?",
      actions: [
        "Delete the worktree (files Build did not create)",
        "Delete branch feat/x",
        "Keep the task as history",
      ],
      confirmLabel: "Delete worktree",
      danger: true,
    });
  });

  it("a Build-created task gets the plain abandon outline", () => {
    expect(abandonConfirm({ adopted: false, branch: "feat/x" })).toEqual({
      title: "Abandon this task?",
      actions: ["Delete the worktree", "Delete branch feat/x", "Keep the task as history"],
      confirmLabel: "Abandon",
      danger: true,
    });
  });
});

describe("merge failure reason extraction", () => {
  it("strips the merge_failed: prefix and trims", () => {
    expect(mergeFailureReason("merge_failed: conflict in a.txt, b.txt")).toBe("conflict in a.txt, b.txt");
    expect(mergeFailureReason("merge_failed:primary checkout is on \"x\"")).toBe('primary checkout is on "x"');
  });

  it("returns null for non-merge errors", () => {
    expect(mergeFailureReason("unknown run_id")).toBeNull();
    expect(mergeFailureReason(null)).toBeNull();
    expect(mergeFailureReason(undefined)).toBeNull();
  });
});

