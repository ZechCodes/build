import { describe, it, expect } from "vitest";
import {
  planArchiveConfirm,
  runFinishWorktree,
  worktreeFinishActions,
  worktreeFinishConfirm,
  worktreeFinishSheetHtml,
} from "../src/core/railDone.js";

const worktree = (over = {}) => ({
  worktree_id: "w1",
  project_id: "p1",
  name: "feature checkout",
  branch: "feat/rail",
  base_branch: "main",
  upstream: "origin/feat/rail",
  dirty_files: 3,
  unpushed: 2,
  ...over,
});

describe("run Done metadata", () => {
  it("uses the checked-out branch and live git status for the shared finish actions", () => {
    expect(runFinishWorktree({
      run_id: "r1",
      project_id: "p1",
      goal: "latex support",
      branch: "stale/branch",
      base_branch: "main",
      stat: {
        branch: "build/latex-renderer-ready",
        upstream: "origin/build/latex-renderer-ready",
        ahead: 0,
        uncommitted: { files_changed: 0 },
      },
    })).toMatchObject({
      run_id: "r1",
      name: "latex support",
      branch: "build/latex-renderer-ready",
      upstream: "origin/build/latex-renderer-ready",
      dirty_files: 0,
      unpushed: 0,
    });
  });
});

describe("worktree Done action matrix", () => {
  it("offers only cleanup for a clean worktree", () => {
    expect(worktreeFinishActions(worktree({ dirty_files: 0 })).map((action) => action.id)).toEqual(["cleanup"]);
  });

  it("offers Push, Merge, and Delete for a dirty tracked feature branch", () => {
    expect(worktreeFinishActions(worktree()).map((action) => action.id)).toEqual(["push", "merge", "delete"]);
  });

  it("omits Push without an upstream and Merge when branch equals base", () => {
    expect(
      worktreeFinishActions(worktree({ upstream: null, branch: "main", base_branch: "main" })).map(
        (action) => action.id,
      ),
    ).toEqual(["delete"]);
  });

  it("allows permanent deletion of a dirty detached worktree", () => {
    expect(
      worktreeFinishActions(worktree({ branch: null, upstream: null })).map((action) => action.id),
    ).toEqual(["delete"]);
  });
});

describe("rail Done confirmations", () => {
  it("explains that archiving moves the plan and docs to Project Archive", () => {
    expect(planArchiveConfirm()).toMatchObject({
      title: "Archive this issue?",
      confirmLabel: "Archive issue",
      danger: false,
    });
    expect(planArchiveConfirm().actions.join(" ")).toContain("Issue and its stage plans to Project Archive");
  });

  it("discloses branch preservation and unpushed commits during cleanup", () => {
    const confirmation = worktreeFinishConfirm("cleanup", worktree({ dirty_files: 0 }));
    expect(confirmation.actions).toContain("Keep branch feat/rail");
    expect(confirmation.actions.join(" ")).toContain("2 unpushed commits");
  });

  it("states permanent loss and attached branch deletion for Delete", () => {
    const confirmation = worktreeFinishConfirm("delete", worktree());
    expect(confirmation.danger).toBe(true);
    expect(confirmation.actions.join(" ")).toContain("Permanently lose 3 uncommitted files");
    expect(confirmation.actions).toContain("Delete branch feat/rail");
  });

  it("does not invent a branch or block Delete for a detached worktree", () => {
    const confirmation = worktreeFinishConfirm("delete", worktree({ branch: null, upstream: null }));
    expect(confirmation.actions).toContain("Delete the detached worktree");
    expect(confirmation.actions.join(" ")).not.toContain("Delete branch");
  });
});

describe("worktree Done chooser markup", () => {
  it("escapes branch and upstream strings", () => {
    const html = worktreeFinishSheetHtml(
      worktree({ branch: '<img src=x onerror="bad">', upstream: "origin/<script>bad</script>" }),
    );
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;");
  });
});
